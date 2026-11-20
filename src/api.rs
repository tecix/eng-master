//! ChatGPT plan inference: `GET /v1/models` and streaming `POST /v1/responses`.
//!
//! Plan tokens require `store: false` and `stream: true`; `max_output_tokens`, `temperature`,
//! `top_p`, `metadata`, `user` and system-role input items are not accepted, so instructions go in
//! `instructions`. Success is only `response.completed`.

use crate::http::{self, HttpError};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read};

const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub id: String,
    pub name: String,
}

pub fn models(base: &str, token: &str) -> Result<Vec<Model>> {
    let data = http::get_json(&format!("{base}/models"), Some(token))?;
    // The plan endpoint lists `models: [{slug, display_name, visibility}]`; accept the
    // standard `data: [{id}]` shape as well.
    let mut out: Vec<Model> = data
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|m| m.get("visibility").and_then(Value::as_str).is_none_or(|v| v == "list"))
        .filter_map(|m| {
            let id = m.get("slug").and_then(Value::as_str)?.to_string();
            let name = m.get("display_name").and_then(Value::as_str).unwrap_or(&id).to_string();
            Some(Model { id, name })
        })
        .collect();
    if out.is_empty() {
        out = data
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|m| m.get("id").and_then(Value::as_str))
            .map(|id| Model { id: id.to_string(), name: id.to_string() })
            .collect();
    }
    Ok(out)
}

pub struct Request<'a> {
    pub model: &'a str,
    pub instructions: &'a str,
    pub input: &'a str,
    pub effort: Option<&'a str>,
    pub schema: Option<Value>,
}

pub fn request_body(req: &Request) -> Value {
    let mut body = json!({
        "model": req.model,
        "instructions": req.instructions,
        "input": [{"role": "user", "content": req.input}],
        "store": false,
        "stream": true,
    });
    if let Some(effort) = req.effort {
        body["reasoning"] = json!({"effort": effort});
    }
    if let Some(schema) = &req.schema {
        body["text"] = json!({"format": {"type": "json_schema", "name": "writing_feedback", "strict": true, "schema": schema}});
    }
    body
}

/// Send one request and return the final output text. `progress` gets the streamed character count.
pub fn respond(base: &str, token: &str, req: &Request, progress: &mut dyn FnMut(usize)) -> Result<String> {
    let mut response = http::agent()
        .post(&format!("{base}/responses"))
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "text/event-stream")
        .header("User-Agent", http::user_agent())
        .send_json(request_body(req))
        .context("could not reach ChatGPT")?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let body = response.body_mut().read_to_string().unwrap_or_default();
        return Err(HttpError::from_body(status, &body).into());
    }
    let reader = BufReader::new(response.into_body().into_reader());
    read_stream(reader, progress)
}

pub fn read_stream(reader: impl Read, progress: &mut dyn FnMut(usize)) -> Result<String> {
    let mut parser = SseParser::default();
    for line in BufReader::new(reader).lines() {
        let line = line.context("the ChatGPT connection was interrupted")?;
        parser.feed_line(&line)?;
        progress(parser.streamed.chars().count());
        if parser.done.is_some() {
            break;
        }
    }
    parser.feed_line("")?; // flush a final frame without a trailing blank line
    parser.finish()
}

#[derive(Default)]
pub struct SseParser {
    data: Vec<String>,
    size: usize,
    streamed: String,
    items: BTreeMap<i64, Value>,
    done: Option<String>,
}

impl SseParser {
    pub fn feed_line(&mut self, line: &str) -> Result<()> {
        self.size += line.len() + 1;
        if self.size > MAX_RESPONSE_BYTES {
            bail!("The ChatGPT response is too large. Select less text.");
        }
        if let Some(data) = line.strip_prefix("data:") {
            self.data.push(data.strip_prefix(' ').unwrap_or(data).to_string());
            return Ok(());
        }
        if !line.is_empty() || self.data.is_empty() {
            return Ok(()); // `event:`, `id:`, comments
        }
        let data = std::mem::take(&mut self.data).join("\n");
        if data == "[DONE]" {
            return Ok(());
        }
        let event: Value = serde_json::from_str(&data).context("unexpected data from ChatGPT")?;
        self.event(&event)
    }

    fn event(&mut self, event: &Value) -> Result<()> {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or_default();
        match kind {
            "response.output_text.delta" => {
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    self.streamed.push_str(delta);
                }
            }
            "error" | "response.failed" => {
                let error = event
                    .pointer("/response/error")
                    .or_else(|| event.get("error"))
                    .cloned()
                    .unwrap_or_else(|| event.clone());
                return Err(HttpError::from_json(0, &json!({"error": error})).into());
            }
            "response.incomplete" => {
                let reason = event
                    .pointer("/response/incomplete_details/reason")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                bail!("ChatGPT returned an incomplete response ({reason}). Try a shorter text.");
            }
            "response.output_item.done" => {
                if let (Some(index), Some(item)) = (event.get("output_index").and_then(Value::as_i64), event.get("item"))
                    && item.get("type").and_then(Value::as_str) == Some("message") {
                        self.items.insert(index, item.clone());
                    }
            }
            "response.completed" => {
                if event.pointer("/response/status").and_then(Value::as_str) != Some("completed") {
                    bail!("ChatGPT did not complete the response.");
                }
                let output: Vec<Value> = match event.pointer("/response/output").and_then(Value::as_array) {
                    Some(list) if !list.is_empty() => list.clone(),
                    _ => self.items.values().cloned().collect(),
                };
                let text: String = output
                    .iter()
                    .filter(|item| item.get("type").and_then(Value::as_str) == Some("message"))
                    .flat_map(|item| item.get("content").and_then(Value::as_array).cloned().unwrap_or_default())
                    .filter_map(|part| match part.get("type").and_then(Value::as_str) {
                        Some("output_text") => part.get("text").and_then(Value::as_str).map(str::to_string),
                        Some("refusal") => part.get("refusal").and_then(Value::as_str).map(str::to_string),
                        _ => None,
                    })
                    .collect();
                self.done = Some(if text.is_empty() { self.streamed.clone() } else { text });
            }
            _ => {}
        }
        Ok(())
    }

    pub fn finish(self) -> Result<String> {
        let text = self
            .done
            .ok_or_else(|| anyhow!("ChatGPT disconnected before finishing. Nothing was changed."))?;
        if text.trim().is_empty() {
            bail!("ChatGPT returned no text.");
        }
        Ok(text.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(stream: &str) -> Result<String> {
        read_stream(stream.as_bytes(), &mut |_| {})
    }

    #[test]
    fn collects_completed_output() {
        let stream = "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"Hel\"}\n\n\
            data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[\
            {\"type\":\"reasoning\"},{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello\"}]}]}}\n\n";
        assert_eq!(run(stream).unwrap(), "Hello");
    }

    #[test]
    fn falls_back_to_items_then_deltas() {
        let stream = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hi\"}\n\n\
            data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hi there\"}]}}\n\n\
            data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[]}}\n\n";
        assert_eq!(run(stream).unwrap(), "Hi there");
        let deltas = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Yo\"}\n\n\
            data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}";
        assert_eq!(run(deltas).unwrap(), "Yo");
    }

    #[test]
    fn reports_failures() {
        let failed = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"subscription_sharing_usage_limit_exceeded\"}}}\n\n";
        let err = run(failed).unwrap_err();
        assert!(err.downcast_ref::<HttpError>().unwrap().is_code(&["subscription_sharing_usage_limit_exceeded"]));
        assert!(err.to_string().contains("usage limit"));
        let incomplete = "data: {\"type\":\"response.incomplete\",\"response\":{\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n";
        assert!(run(incomplete).unwrap_err().to_string().contains("max_output_tokens"));
        assert!(run("data: {\"type\":\"response.output_text.delta\",\"delta\":\"x\"}\n\n").is_err());
    }

    /// Serve one canned HTTP response on a local port; returns the base URL and the request.
    fn mock(status: &str, content_type: &str, body: &'static str) -> (String, std::thread::JoinHandle<String>) {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let status = status.to_string();
        let content_type = content_type.to_string();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = stream.read(&mut buf).unwrap();
                request.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&request);
                if let Some(head_end) = text.find("\r\n\r\n") {
                    let length = text[..head_end]
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap()))
                        .unwrap_or(0);
                    let chunked = text[..head_end].to_ascii_lowercase().contains("transfer-encoding: chunked");
                    if (chunked && text.ends_with("0\r\n\r\n")) || (!chunked && request.len() >= head_end + 4 + length) {
                        break;
                    }
                }
            }
            write!(stream, "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nConnection: close\r\n\r\n{body}").unwrap();
            String::from_utf8_lossy(&request).to_string()
        });
        (base, handle)
    }

    #[test]
    fn streams_a_response_from_a_server() {
        let (base, server) = mock(
            "200 OK",
            "text/event-stream",
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"{\\\"text\\\":\"}\n\n\
             data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"{\\\"text\\\":\\\"Hi\\\",\\\"mistakes\\\":[],\\\"tips\\\":[]}\"}]}]}}\n\n",
        );
        let schema = Some(crate::actions::schema());
        let req = Request { model: "gpt-x", instructions: "be nice", input: "hi", effort: None, schema };
        let mut seen = 0;
        let text = respond(&base, "tok", &req, &mut |n| seen = n).unwrap();
        assert_eq!(text, "{\"text\":\"Hi\",\"mistakes\":[],\"tips\":[]}");
        assert!(seen > 0);
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /v1/responses "));
        assert!(request.to_ascii_lowercase().contains("authorization: bearer tok"));
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
        assert_eq!(body["text"]["format"]["type"], "json_schema");
        assert_eq!(body["input"][0]["role"], "user");
    }

    #[test]
    fn http_errors_keep_the_rejected_param() {
        let (base, _server) = mock(
            "400 Bad Request",
            "application/json",
            "{\"error\":{\"message\":\"Unsupported parameter: text.format\",\"param\":\"text.format\",\"code\":\"unsupported_parameter\"}}",
        );
        let req = Request { model: "m", instructions: "i", input: "t", effort: None, schema: None };
        let err = respond(&base, "tok", &req, &mut |_| {}).unwrap_err();
        let http = err.downcast_ref::<HttpError>().unwrap();
        assert!(http.rejects_param("text"));
        assert!(!http.rejects_param("reasoning"));
    }

    #[test]
    fn lists_plan_models() {
        let (base, _server) = mock(
            "200 OK",
            "application/json",
            "{\"models\":[{\"slug\":\"gpt-a\",\"display_name\":\"GPT A\",\"visibility\":\"list\"},{\"slug\":\"hidden\",\"visibility\":\"hide\"}]}",
        );
        assert_eq!(models(&base, "tok").unwrap(), vec![Model { id: "gpt-a".into(), name: "GPT A".into() }]);
    }

    #[test]
    fn body_has_required_fields_and_no_unsupported_ones() {
        let body = request_body(&Request { model: "m", instructions: "i", input: "t", effort: Some("low"), schema: None });
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
        assert_eq!(body["reasoning"]["effort"], "low");
        for key in ["max_output_tokens", "temperature", "top_p", "metadata", "user", "text"] {
            assert!(body.get(key).is_none(), "{key}");
        }
    }
}
