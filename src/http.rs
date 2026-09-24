//! In-process HTTP: one place for timeouts, proxies, retries and SSE.
//!
//! Secrets never touch argv or a shared temp file — headers are set on the
//! request object inside this process.

use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::io::{BufRead, BufReader, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

pub const USER_AGENT: &str = concat!("ice/", env!("CARGO_PKG_VERSION"));

fn build_agent(proxy: Option<ureq::Proxy>) -> ureq::Agent {
    let mut b = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        // Long read timeout: a model can think for a while between chunks.
        .timeout_read(Duration::from_secs(600))
        .user_agent(USER_AGENT);
    if let Some(p) = proxy {
        b = b.proxy(p);
    }
    b.build()
}

/// The agent for `url`: through HTTPS_PROXY unless the host is local or
/// listed in NO_PROXY.
fn agent_for(url: &str) -> &'static ureq::Agent {
    static PROXIED: OnceLock<ureq::Agent> = OnceLock::new();
    static DIRECT: OnceLock<ureq::Agent> = OnceLock::new();
    let host = crate::tools::host_of(url);
    let proxy = proxy_from_env();
    if proxy.is_none() || bypass_proxy(&host) {
        DIRECT.get_or_init(|| build_agent(None))
    } else {
        PROXIED.get_or_init(|| build_agent(proxy_from_env()))
    }
}

fn bypass_proxy(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host == "localhost" || host.starts_with("127.") || host == "::1" || host == "0.0.0.0" {
        return true;
    }
    let list = std::env::var("NO_PROXY")
        .or_else(|_| std::env::var("no_proxy"))
        .unwrap_or_default();
    no_proxy_matches(&list, host)
}

fn proxy_from_env() -> Option<ureq::Proxy> {
    [
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
        "HTTP_PROXY",
        "http_proxy",
    ]
    .iter()
    .filter_map(|k| std::env::var(k).ok())
    .find(|v| !v.trim().is_empty())
    .and_then(|v| ureq::Proxy::new(v.trim()).ok())
}

pub fn no_proxy_matches(list: &str, host: &str) -> bool {
    list.split(',')
        .map(|e| e.trim())
        .filter(|e| !e.is_empty())
        .any(|e| {
            if e == "*" {
                return true;
            }
            let e = e.split(':').next().unwrap_or(e);
            let e = e.trim_start_matches("*.");
            let e = e.trim_start_matches('.');
            host == e || host.ends_with(&format!(".{e}"))
        })
}

/// An HTTP failure with its status code, so callers can react to 401/429/5xx.
#[derive(Debug)]
pub struct HttpError {
    pub status: u16,
    pub body: String,
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = extract_error_message(&self.body);
        match self.status {
            401 | 403 => write!(
                f,
                "authentication failed ({}): {msg} — check your API key (/login)",
                self.status
            ),
            404 => write!(
                f,
                "not found (404): {msg} — check the model name (/model) and base URL"
            ),
            429 => write!(f, "rate limited (429): {msg}"),
            s if s >= 500 => write!(f, "provider error ({s}): {msg}"),
            s => write!(f, "API error ({s}): {msg}"),
        }
    }
}

impl std::error::Error for HttpError {}

/// Pull a human message out of the usual `{"error":{"message":…}}` shapes.
pub fn extract_error_message(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        let e = v.get("error").unwrap_or(&v);
        if let Some(m) = e.get("message").and_then(|m| m.as_str()) {
            return m.to_string();
        }
        if let Some(m) = e.as_str() {
            return m.to_string();
        }
    }
    let t = body.trim();
    if t.is_empty() {
        "(empty response)".into()
    } else {
        t.chars().take(300).collect()
    }
}

fn retryable(status: u16) -> bool {
    matches!(status, 408 | 409 | 425 | 429 | 500 | 502 | 503 | 504 | 529)
}

/// Exponential backoff with jitter: ~1s, 2s, 4s, 8s … capped at 32s.
fn backoff(attempt: u32, retry_after: Option<u64>) -> Duration {
    if let Some(s) = retry_after {
        return Duration::from_secs(s.min(60));
    }
    let base = 1000u64 << attempt.min(5);
    let jitter = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_millis() as u64)
        .unwrap_or(0))
        % 250;
    Duration::from_millis(base.min(32_000) + jitter)
}

fn sleep_cancellable(d: Duration, cancel: Option<&AtomicBool>) -> Result<()> {
    let step = Duration::from_millis(50);
    let mut left = d;
    while left > Duration::ZERO {
        if cancel.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false) {
            bail!(Interrupted);
        }
        let s = left.min(step);
        std::thread::sleep(s);
        left = left.saturating_sub(s);
    }
    Ok(())
}

/// Marker error: the user interrupted (Esc / Ctrl+C).
#[derive(Debug)]
pub struct Interrupted;
impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Interrupted by user")
    }
}
impl std::error::Error for Interrupted {}

pub fn is_interrupted(e: &anyhow::Error) -> bool {
    e.downcast_ref::<Interrupted>().is_some()
}

pub struct Request<'a> {
    pub url: &'a str,
    pub headers: Vec<(String, String)>,
    pub body: &'a Value,
    pub max_retries: u32,
}

/// POST JSON and return the response reader once a 2xx arrives, retrying
/// transient failures (429/5xx/network) with backoff. `on_retry` is told
/// about each wait so the UI can show "retrying in 4s…".
pub fn post(
    req: &Request,
    cancel: Option<&AtomicBool>,
    on_retry: &dyn Fn(u32, &str, Duration),
) -> Result<ureq::Response> {
    let payload = req.body.to_string();
    let mut attempt = 0;
    loop {
        if cancel.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false) {
            bail!(Interrupted);
        }
        let mut r = agent_for(req.url)
            .post(req.url)
            .set("content-type", "application/json");
        for (k, v) in &req.headers {
            r = r.set(k, v);
        }
        match r.send_string(&payload) {
            Ok(resp) => return Ok(resp),
            Err(ureq::Error::Status(code, resp)) => {
                let retry_after = resp
                    .header("retry-after")
                    .and_then(|s| s.trim().parse::<u64>().ok());
                let body = resp.into_string().unwrap_or_default();
                if retryable(code) && attempt < req.max_retries {
                    let wait = backoff(attempt, retry_after);
                    on_retry(
                        attempt + 1,
                        &HttpError {
                            status: code,
                            body: body.clone(),
                        }
                        .to_string(),
                        wait,
                    );
                    sleep_cancellable(wait, cancel)?;
                    attempt += 1;
                    continue;
                }
                return Err(HttpError { status: code, body }.into());
            }
            Err(ureq::Error::Transport(t)) => {
                if attempt < req.max_retries {
                    let wait = backoff(attempt, None);
                    on_retry(attempt + 1, &t.to_string(), wait);
                    sleep_cancellable(wait, cancel)?;
                    attempt += 1;
                    continue;
                }
                return Err(anyhow!("network error: {t}"));
            }
        }
    }
}

/// Iterate a `text/event-stream` body, calling `on_event(event, data)` for
/// each complete event. Stops early (with `Interrupted`) when cancelled.
pub fn read_sse(
    resp: ureq::Response,
    cancel: Option<&AtomicBool>,
    mut on_event: impl FnMut(&str, &str) -> Result<()>,
) -> Result<()> {
    let reader = BufReader::new(resp.into_reader());
    let mut event = String::new();
    let mut data = String::new();
    for line in reader.lines() {
        if cancel.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false) {
            bail!(Interrupted);
        }
        let line = line.map_err(|e| anyhow!("stream interrupted: {e}"))?;
        if line.is_empty() {
            if !data.is_empty() {
                on_event(&event, &data)?;
            }
            event.clear();
            data.clear();
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line.as_str(), ""),
        };
        match field {
            "event" => event = value.to_string(),
            "data" => {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(value);
            }
            _ => {}
        }
    }
    if !data.is_empty() {
        on_event(&event, &data)?;
    }
    Ok(())
}

/// Simple GET returning the body as text (bounded to `max_bytes`).
pub fn get_text(
    url: &str,
    headers: &[(&str, &str)],
    timeout: Duration,
    max_bytes: u64,
) -> Result<(String, String)> {
    let mut r = agent_for(url).get(url).timeout(timeout);
    for (k, v) in headers {
        r = r.set(k, v);
    }
    match r.call() {
        Ok(resp) => {
            let ctype = resp.content_type().to_string();
            let mut buf = Vec::new();
            resp.into_reader().take(max_bytes).read_to_end(&mut buf)?;
            Ok((ctype, String::from_utf8_lossy(&buf).into_owned()))
        }
        Err(ureq::Error::Status(code, resp)) => Err(HttpError {
            status: code,
            body: resp.into_string().unwrap_or_default(),
        }
        .into()),
        Err(ureq::Error::Transport(t)) => Err(anyhow!("network error: {t}")),
    }
}

/// POST an `application/x-www-form-urlencoded` body and parse JSON (OAuth
/// token endpoints). Error bodies surface as HttpError.
pub fn post_form(url: &str, form: &[(&str, String)]) -> Result<Value> {
    let pairs: Vec<(&str, &str)> = form.iter().map(|(k, v)| (*k, v.as_str())).collect();
    match agent_for(url)
        .post(url)
        .set("accept", "application/json")
        .send_form(&pairs)
    {
        Ok(resp) => {
            let body = resp.into_string()?;
            serde_json::from_str(&body).map_err(|e| anyhow!("invalid JSON from {url}: {e}"))
        }
        Err(ureq::Error::Status(code, resp)) => Err(HttpError {
            status: code,
            body: resp.into_string().unwrap_or_default(),
        }
        .into()),
        Err(ureq::Error::Transport(t)) => Err(anyhow!("network error: {t}")),
    }
}

/// GET JSON.
pub fn get_json(url: &str, headers: &[(&str, &str)], timeout: Duration) -> Result<Value> {
    let (_, body) = get_text(url, headers, timeout, 16 << 20)?;
    serde_json::from_str(&body).map_err(|e| anyhow!("invalid JSON from {url}: {e}"))
}

/// Download to bytes (for self-update).
pub fn get_bytes(url: &str, timeout: Duration) -> Result<Vec<u8>> {
    match agent_for(url).get(url).timeout(timeout).call() {
        Ok(resp) => {
            let mut buf = Vec::new();
            resp.into_reader().take(256 << 20).read_to_end(&mut buf)?;
            Ok(buf)
        }
        Err(ureq::Error::Status(code, resp)) => Err(HttpError {
            status: code,
            body: resp.into_string().unwrap_or_default(),
        }
        .into()),
        Err(ureq::Error::Transport(t)) => Err(anyhow!("network error: {t}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_are_extracted() {
        assert_eq!(
            extract_error_message(r#"{"error":{"type":"x","message":"bad key"}}"#),
            "bad key"
        );
        assert_eq!(extract_error_message(r#"{"error":"nope"}"#), "nope");
        assert_eq!(extract_error_message(""), "(empty response)");
        let e = HttpError {
            status: 401,
            body: r#"{"error":{"message":"invalid x-api-key"}}"#.into(),
        };
        assert!(e.to_string().contains("invalid x-api-key"));
        assert!(e.to_string().contains("/login"));
    }

    #[test]
    fn no_proxy_lists() {
        assert!(no_proxy_matches(
            "localhost,.internal.corp",
            "api.internal.corp"
        ));
        assert!(no_proxy_matches("*.example.com", "x.example.com"));
        assert!(no_proxy_matches("example.com:443", "example.com"));
        assert!(!no_proxy_matches("example.com", "notexample.com"));
        assert!(no_proxy_matches("*", "anything"));
        assert!(bypass_proxy("127.0.0.1"));
    }

    #[test]
    fn backoff_grows_and_honours_retry_after() {
        assert!(backoff(0, None) < backoff(3, None));
        assert_eq!(backoff(0, Some(7)), Duration::from_secs(7));
        assert!(backoff(20, None) <= Duration::from_millis(32_250));
    }
}
