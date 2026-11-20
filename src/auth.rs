//! "Sign in with ChatGPT" for open-source, locally hosted apps (OpenAI SIWC token sharing).
//!
//! Same flow as Plyph and the OpenAI docs at
//! <https://developers.openai.com/siwc/token-sharing-open-source>:
//! first sign-in registers this machine with `client_id=dynamic_agent_client` plus a stable
//! `ext_agent_host_id`; the callback returns the issued `oaiapp_…` client ID, which is saved and
//! used for token exchange, refresh and later sign-ins. PKCE (S256), state and nonce protect the
//! loopback redirect. Tokens live in the GNOME keyring.

use crate::http::{self, HttpError};
use crate::{keyring, paths};
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const ISSUER: &str = "https://auth.openai.com";
pub const RESOURCE: &str = "https://api.openai.com/v1";
const SCOPES: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
const REGISTRATION_CLIENT: &str = "dynamic_agent_client";
const AGENT_NAME: &str = "en-Master";
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Credentials {
    /// Stable per-machine `ext_agent_host_id` (`urn:uuid:…`).
    pub host_id: String,
    /// Issued `oaiapp_…` client ID. Empty until the first registration completes.
    pub client_id: String,
    pub subject: String,
    pub email: String,
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds.
    pub expires_at: i64,
    pub scopes: Vec<String>,
}

impl Credentials {
    fn has_plan_access(&self) -> bool {
        self.scopes.iter().any(|s| s == PLAN_SCOPE)
    }

    fn clear_tokens(&mut self) {
        self.access_token.clear();
        self.refresh_token.clear();
        self.expires_at = 0;
    }
}

/// The user has to (re)connect their ChatGPT plan.
#[derive(Debug)]
pub struct NotSignedIn(pub &'static str);

impl fmt::Display for NotSignedIn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for NotSignedIn {}

pub enum Status {
    SignedOut,
    SignedIn { email: String, plan_access: bool },
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn load() -> Result<Credentials> {
    match keyring::read()? {
        None => Ok(Credentials::default()),
        Some(json) => serde_json::from_str(&json).context(
            "the saved ChatGPT connection is invalid; delete the \"en-Master\" item in Passwords and Keys",
        ),
    }
}

fn save(creds: &Credentials) -> Result<()> {
    keyring::write(&serde_json::to_string(creds)?)
}

/// Serializes credential reads/writes across processes so a rotating refresh token never races.
fn lock() -> Result<File> {
    let dir = paths::data_dir();
    paths::ensure_dir(&dir)?;
    let file = OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("auth.lock"))?;
    file.lock()?;
    Ok(file)
}

pub fn status() -> Result<Status> {
    let creds = load()?;
    if creds.refresh_token.is_empty() {
        return Ok(Status::SignedOut);
    }
    Ok(Status::SignedIn { email: creds.email.clone(), plan_access: creds.has_plan_access() })
}

fn random_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn form(pairs: &[(&str, &str)]) -> String {
    url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish()
}

fn open_browser(url: &str) {
    eprintln!("Opening your browser to sign in. If it doesn't open, visit:\n{url}");
    let _ = std::process::Command::new("xdg-open")
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Run the browser sign-in. Blocks until the callback arrives, `cancel` is set, or 3 minutes pass.
/// `register_again` forgets the issued client ID so this machine registers anew (for example to
/// show the app's current name in ChatGPT settings); the old token is revoked first.
pub fn sign_in(cancel: &AtomicBool, register_again: bool) -> Result<Credentials> {
    if register_again {
        let _ = sign_out();
    }
    let previous = {
        let _guard = lock()?;
        let mut creds = load()?;
        if register_again && !creds.client_id.is_empty() {
            creds = Credentials { host_id: creds.host_id, ..Default::default() };
            save(&creds)?;
        }
        if creds.host_id.is_empty() {
            creds.host_id = format!("urn:uuid:{}", uuid::Uuid::new_v4());
            save(&creds)?;
        }
        creds
    };

    let state = random_token()?;
    let nonce = random_token()?;
    let verifier = random_token()?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));

    // The docs require the 127.0.0.1 loopback (not "localhost"); only the port may vary.
    let listener = TcpListener::bind("127.0.0.1:0").context("could not open the sign-in callback port")?;
    let port = listener.local_addr()?.port();
    let redirect = format!("http://127.0.0.1:{port}/auth/callback");

    let client_id = if previous.client_id.is_empty() { REGISTRATION_CLIENT } else { previous.client_id.as_str() };
    let mut params = vec![
        ("client_id", client_id),
        ("ext_agent_host_id", previous.host_id.as_str()),
        ("response_type", "code"),
        ("redirect_uri", redirect.as_str()),
        ("scope", SCOPES),
        ("resource", RESOURCE),
        ("state", state.as_str()),
        ("nonce", nonce.as_str()),
        ("code_challenge_method", "S256"),
        ("code_challenge", challenge.as_str()),
    ];
    if previous.client_id.is_empty() {
        params.push(("agent_name_hint", AGENT_NAME));
    } else if !previous.email.is_empty() {
        params.push(("login_hint", previous.email.as_str()));
    }
    open_browser(&format!("{ISSUER}/api/accounts/authorize?{}", form(&params)));

    let query = wait_for_callback(&listener, port, &state, cancel)?;
    drop(listener);
    let (issued, code) = callback_result(&query, &previous.client_id)?;

    let tokens = http::post_form(
        &format!("{ISSUER}/api/accounts/oauth/token"),
        &[
            ("grant_type", "authorization_code"),
            ("client_id", issued.as_str()),
            ("code", code.as_str()),
            ("code_verifier", verifier.as_str()),
            ("redirect_uri", redirect.as_str()),
            ("resource", RESOURCE),
        ],
    )?;

    let _guard = lock()?;
    let mut creds = load()?;
    let claims = id_token_claims(&tokens, &issued, Some(&nonce))?
        .ok_or_else(|| anyhow!("ChatGPT did not return an identity token. Try signing in again."))?;
    let subject = claims.get("sub").and_then(Value::as_str).unwrap_or_default().to_string();
    if !creds.subject.is_empty() && creds.client_id == issued && creds.subject != subject {
        bail!("You signed in with a different ChatGPT account. Sign out first to switch accounts.");
    }
    apply_tokens(&mut creds, &tokens)?;
    creds.client_id = issued;
    creds.subject = subject;
    if claims.get("email_verified").and_then(Value::as_bool) != Some(false) {
        creds.email = claims.get("email").and_then(Value::as_str).unwrap_or_default().to_string();
    }
    save(&creds)?;
    if !creds.has_plan_access() {
        bail!(NotSignedIn("ChatGPT plan access was not granted. Continue with ChatGPT and allow plan usage."));
    }
    Ok(creds)
}

fn wait_for_callback(
    listener: &TcpListener,
    port: u16,
    state: &str,
    cancel: &AtomicBool,
) -> Result<HashMap<String, String>> {
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + SIGN_IN_TIMEOUT;
    loop {
        if cancel.load(Ordering::Relaxed) {
            bail!("Sign-in cancelled.");
        }
        if Instant::now() > deadline {
            bail!("ChatGPT sign-in timed out. Try again.");
        }
        match listener.accept() {
            Ok((stream, _)) => {
                if let Some(query) = handle_callback(stream, port, state) {
                    return Ok(query);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(e.into()),
        }
    }
}

/// Answer one loopback request. Returns the query when it is the expected callback.
fn handle_callback(mut stream: TcpStream, port: u16, state: &str) -> Option<HashMap<String, String>> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 16 * 1024 {
        let n = stream.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        head.extend_from_slice(&buf[..n]);
    }
    let head = String::from_utf8_lossy(&head);
    let mut lines = head.split("\r\n");
    let mut request = lines.next().unwrap_or_default().split(' ');
    let (method, target) = (request.next().unwrap_or_default(), request.next().unwrap_or_default());
    let host = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("host"))
        .map(|(_, value)| value.trim().to_string())
        .unwrap_or_default();
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let query: HashMap<String, String> = url::form_urlencoded::parse(query.as_bytes()).into_owned().collect();
    let valid = method == "GET"
        && path == "/auth/callback"
        && host == format!("127.0.0.1:{port}")
        && query.get("state").map(String::as_str) == Some(state);

    let (status, body) = if valid {
        ("200 OK", "en-Master is connected to ChatGPT. You can close this tab.")
    } else {
        ("400 Bad Request", "Invalid sign-in callback.")
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\n\
         Content-Security-Policy: default-src 'none'\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    valid.then_some(query)
}

/// Validate the callback query and return (issued client ID, authorization code).
fn callback_result(query: &HashMap<String, String>, saved_client: &str) -> Result<(String, String)> {
    if let Some(error) = query.get("error") {
        if error == "access_denied" {
            bail!("ChatGPT sign-in was declined.");
        }
        bail!("ChatGPT sign-in failed ({error}).");
    }
    // A first registration returns the issued client ID; a re-authorization may omit it.
    let issued = query.get("client_id").map(String::as_str).filter(|s| !s.is_empty()).unwrap_or(saved_client);
    let valid_id = !issued.is_empty()
        && issued != REGISTRATION_CLIENT
        && issued.len() <= 256
        && issued.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    let code = query.get("code").map(String::as_str).unwrap_or_default();
    if !valid_id || (!saved_client.is_empty() && issued != saved_client) || code.is_empty() {
        bail!("ChatGPT registration was incomplete. Try signing in again.");
    }
    Ok((issued.to_string(), code.to_string()))
}

/// Check the ID token's claims.
///
/// The signature is not verified: the token comes straight from the token endpoint over TLS,
/// which OpenID Connect Core §3.1.3.7 allows in place of signature validation.
fn id_token_claims(tokens: &Value, client_id: &str, nonce: Option<&str>) -> Result<Option<Value>> {
    let Some(token) = tokens.get("id_token").and_then(Value::as_str) else { return Ok(None) };
    let payload = token.split('.').nth(1).ok_or_else(|| anyhow!("invalid identity token"))?;
    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload.trim_end_matches('='))?)?;
    let audience_ok = match claims.get("aud") {
        Some(Value::String(aud)) => aud == client_id,
        Some(Value::Array(list)) => list.iter().any(|a| a.as_str() == Some(client_id)),
        _ => false,
    };
    let fresh = claims.get("exp").and_then(Value::as_i64).is_some_and(|exp| exp > now());
    let nonce_ok = nonce.is_none_or(|n| claims.get("nonce").and_then(Value::as_str) == Some(n));
    let sub_ok = claims.get("sub").and_then(Value::as_str).is_some_and(|s| !s.is_empty());
    if claims.get("iss").and_then(Value::as_str) != Some(ISSUER) || !audience_ok || !fresh || !nonce_ok || !sub_ok {
        bail!("The ChatGPT sign-in identity could not be verified. Please sign in again.");
    }
    Ok(Some(claims))
}

fn apply_tokens(creds: &mut Credentials, tokens: &Value) -> Result<()> {
    let text = |key: &str| tokens.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    let expires_in = tokens.get("expires_in").and_then(Value::as_i64).unwrap_or(0);
    let access = text("access_token");
    let refresh = text("refresh_token");
    if !text("token_type").eq_ignore_ascii_case("bearer")
        || access.is_empty()
        || expires_in <= 0
        || (refresh.is_empty() && creds.refresh_token.is_empty())
    {
        bail!("ChatGPT returned an incomplete connection. Please sign in again.");
    }
    creds.access_token = access;
    if !refresh.is_empty() {
        creds.refresh_token = refresh; // refresh tokens rotate: always keep the newest
    }
    creds.expires_at = now() + expires_in;
    if let Some(scope) = tokens.get("scope").and_then(Value::as_str) {
        creds.scopes = scope.split_whitespace().map(str::to_string).collect();
    }
    Ok(())
}

/// A valid access token, refreshed when it expires within a minute.
pub fn access_token() -> Result<String> {
    let _guard = lock()?;
    let mut creds = load()?;
    if creds.refresh_token.is_empty() {
        bail!(NotSignedIn("Continue with ChatGPT to connect your plan."));
    }
    if !creds.has_plan_access() {
        bail!(NotSignedIn("ChatGPT plan access was not granted. Continue with ChatGPT and allow plan usage."));
    }
    if creds.access_token.is_empty() || creds.expires_at < now() + 60 {
        let result = http::post_form(
            &format!("{ISSUER}/api/accounts/oauth/token"),
            &[
                ("grant_type", "refresh_token"),
                ("client_id", creds.client_id.as_str()),
                ("refresh_token", creds.refresh_token.as_str()),
                ("resource", RESOURCE),
            ],
        );
        match result {
            Ok(tokens) => {
                if let Some(claims) = id_token_claims(&tokens, &creds.client_id, None)?
                    && claims.get("sub").and_then(Value::as_str) != Some(creds.subject.as_str()) {
                        bail!("The ChatGPT account changed. Sign in again.");
                    }
                apply_tokens(&mut creds, &tokens)?;
                save(&creds)?;
            }
            Err(e) => {
                let expired = e
                    .downcast_ref::<HttpError>()
                    .is_some_and(|h| h.is_code(&["invalid_grant", "refresh_token_reused", "invalid_client"]));
                if expired {
                    creds.clear_tokens();
                    save(&creds)?;
                    bail!(NotSignedIn("Your ChatGPT connection expired. Continue with ChatGPT to sign in again."));
                }
                return Err(e);
            }
        }
    }
    Ok(creds.access_token)
}

/// Revoke the refresh token and forget the tokens. The machine registration (host and client ID)
/// is kept so the next sign-in reuses it. Returns whether revocation was confirmed.
pub fn sign_out() -> Result<bool> {
    let _guard = lock()?;
    let mut creds = load()?;
    if creds.refresh_token.is_empty() {
        return Ok(true);
    }
    let revoked = (|| -> Result<()> {
        let discovery = http::get_json(&format!("{ISSUER}/.well-known/openid-configuration"), None)?;
        let endpoint = discovery
            .get("revocation_endpoint")
            .and_then(Value::as_str)
            .filter(|url| url.starts_with(&format!("{ISSUER}/")))
            .ok_or_else(|| anyhow!("no revocation endpoint"))?;
        http::post_form(
            endpoint,
            &[
                ("token", creds.refresh_token.as_str()),
                ("token_type_hint", "refresh_token"),
                ("client_id", creds.client_id.as_str()),
            ],
        )?;
        Ok(())
    })()
    .is_ok();
    creds.clear_tokens();
    save(&creds)?;
    Ok(revoked)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn first_registration_uses_issued_client_id() {
        let q = query(&[("code", "abc"), ("client_id", "oaiapp_123"), ("state", "s")]);
        assert_eq!(callback_result(&q, "").unwrap(), ("oaiapp_123".into(), "abc".into()));
    }

    #[test]
    fn reauthorization_may_omit_client_id() {
        let q = query(&[("code", "abc")]);
        assert_eq!(callback_result(&q, "oaiapp_9").unwrap().0, "oaiapp_9");
    }

    #[test]
    fn rejects_registration_client_changed_client_and_errors() {
        assert!(callback_result(&query(&[("code", "a"), ("client_id", "dynamic_agent_client")]), "").is_err());
        assert!(callback_result(&query(&[("code", "a"), ("client_id", "oaiapp_2")]), "oaiapp_1").is_err());
        assert!(callback_result(&query(&[("code", "a")]), "").is_err());
        assert!(callback_result(&query(&[("error", "access_denied")]), "").is_err());
        assert!(callback_result(&query(&[("client_id", "oaiapp_1")]), "").is_err());
    }

    fn jwt(claims: Value) -> String {
        format!("e30.{}.sig", URL_SAFE_NO_PAD.encode(claims.to_string()))
    }

    #[test]
    fn id_token_claims_are_checked() {
        let good = serde_json::json!({"iss": ISSUER, "aud": "oaiapp_1", "exp": now() + 600, "sub": "user-1", "nonce": "n"});
        let tokens = serde_json::json!({"id_token": jwt(good.clone())});
        assert!(id_token_claims(&tokens, "oaiapp_1", Some("n")).unwrap().is_some());
        assert!(id_token_claims(&tokens, "oaiapp_1", Some("other")).is_err());
        assert!(id_token_claims(&tokens, "oaiapp_2", Some("n")).is_err());
        let mut expired = good;
        expired["exp"] = (now() - 1).into();
        assert!(id_token_claims(&serde_json::json!({"id_token": jwt(expired)}), "oaiapp_1", None).is_err());
    }

    #[test]
    fn tokens_rotate_and_require_bearer() {
        let mut creds = Credentials { refresh_token: "old".into(), ..Default::default() };
        let tokens = serde_json::json!({"token_type": "Bearer", "access_token": "a", "refresh_token": "new",
            "expires_in": 3600, "scope": "openid chatgpt.tokens.use.direct"});
        apply_tokens(&mut creds, &tokens).unwrap();
        assert_eq!(creds.refresh_token, "new");
        assert!(creds.has_plan_access());
        assert!(apply_tokens(&mut creds, &serde_json::json!({"token_type": "mac", "access_token": "a", "expires_in": 1})).is_err());
    }
}
