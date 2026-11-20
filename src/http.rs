//! Small HTTP helpers and the error type for ChatGPT plan requests.

use anyhow::{Context, Result};
use serde_json::Value;
use std::fmt;
use std::time::Duration;

pub const MANAGE_USAGE_URL: &str = "https://chatgpt.com/settings/usage";
const USER_AGENT: &str = concat!("enmaster/", env!("CARGO_PKG_VERSION"));

pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(180)))
        .build()
        .into()
}

/// A non-2xx answer from auth.openai.com or api.openai.com.
#[derive(Debug, Clone, Default)]
pub struct HttpError {
    pub status: u16,
    pub code: Option<String>,
    pub param: Option<String>,
    pub message: Option<String>,
}

impl HttpError {
    pub fn from_body(status: u16, body: &str) -> Self {
        let json: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        Self::from_json(status, &json)
    }

    pub fn from_json(status: u16, json: &Value) -> Self {
        let error = json.get("error").unwrap_or(json);
        let text = |v: Option<&Value>| v.and_then(Value::as_str).map(str::to_string);
        let code = match error {
            Value::String(s) => Some(s.clone()),
            _ => text(error.get("code")).or_else(|| text(error.get("type"))),
        };
        Self {
            status,
            code,
            param: text(error.get("param")),
            message: text(error.get("message")).or_else(|| text(json.get("error_description"))),
        }
    }

    pub fn is_code(&self, codes: &[&str]) -> bool {
        self.code.as_deref().is_some_and(|c| codes.contains(&c))
    }

    /// True when a 400 complains about a request field starting with `prefix`.
    pub fn rejects_param(&self, prefix: &str) -> bool {
        self.status == 400
            && (self.param.as_deref().is_some_and(|p| p.starts_with(prefix))
                || self.message.as_deref().is_some_and(|m| m.contains(prefix)))
    }
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_code(&["subscription_sharing_usage_limit_exceeded", "subscription_sharing_usage_unavailable"])
            || self.status == 429
        {
            return write!(f, "Your ChatGPT plan usage limit was reached. Check Manage usage: {MANAGE_USAGE_URL}");
        }
        if self.is_code(&["subscription_sharing_user_not_eligible"]) || self.status == 403 {
            return write!(f, "ChatGPT plan usage isn't available for this account or workspace.");
        }
        if self.is_code(&["invalid_grant"]) || self.status == 401 {
            return write!(f, "Your ChatGPT connection expired. Continue with ChatGPT to sign in again.");
        }
        write!(f, "ChatGPT request failed (HTTP {}", self.status)?;
        if let Some(code) = &self.code {
            write!(f, ", {code}")?;
        }
        write!(f, ")")?;
        if let Some(message) = &self.message {
            write!(f, ": {message}")?;
        }
        Ok(())
    }
}

impl std::error::Error for HttpError {}

fn finish(mut response: ureq::http::Response<ureq::Body>) -> Result<Value> {
    let status = response.status().as_u16();
    let body = response.body_mut().read_to_string().unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(HttpError::from_body(status, &body).into());
    }
    if body.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&body).context("unexpected reply from ChatGPT")
}

pub fn post_form(url: &str, form: &[(&str, &str)]) -> Result<Value> {
    let response = agent()
        .post(url)
        .header("User-Agent", USER_AGENT)
        .send_form(form.iter().copied())
        .with_context(|| format!("could not reach {}", host(url)))?;
    finish(response)
}

pub fn get_json(url: &str, bearer: Option<&str>) -> Result<Value> {
    let mut request = agent().get(url).header("User-Agent", USER_AGENT);
    if let Some(token) = bearer {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    let response = request.call().with_context(|| format!("could not reach {}", host(url)))?;
    finish(response)
}

pub fn user_agent() -> &'static str {
    USER_AGENT
}

fn host(url: &str) -> &str {
    url.split("://").nth(1).and_then(|rest| rest.split('/').next()).unwrap_or(url)
}
