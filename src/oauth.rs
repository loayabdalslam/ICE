//! Browser sign-in with OAuth 2.0 + PKCE (RFC 7636, S256), for every
//! provider:
//!
//! - OpenRouter works out of the box: its public PKCE flow needs no client
//!   registration and returns an API key.
//! - Google Gemini, and any other provider or OpenAI-compatible gateway, use
//!   standard authorization-code + PKCE with an OAuth app *you* register
//!   (client id, optional secret, URLs), configured in settings:
//!
//! ```json
//! {"oauth": {"gemini": {"clientId": "…apps.googleusercontent.com", "clientSecret": "…"},
//!            "custom": {"authorizeUrl": "https://sso.example.com/authorize",
//!                       "tokenUrl": "https://sso.example.com/token",
//!                       "clientId": "ice", "scopes": ["openid", "offline_access"]}}}
//! ```
//!
//! (or ICE_OAUTH_<PROVIDER>_CLIENT_ID / _CLIENT_SECRET / _AUTHORIZE_URL /
//! _TOKEN_URL / _SCOPES environment variables). ICE never borrows another
//! product's OAuth client id. Access tokens are stored in
//! ~/.ice/credentials.json (0600) and refreshed automatically.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Exchange {
    /// Standard OAuth 2.0 token endpoint (form-encoded, returns tokens).
    OAuth2,
    /// OpenRouter: POST {code, code_verifier} → {key}.
    OpenRouterKey,
}

#[derive(Clone, Debug)]
pub struct OAuthConfig {
    pub authorize_url: String,
    pub token_url: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub scopes: Vec<String>,
    pub exchange: Exchange,
    /// Fixed loopback port (0 = any free port).
    pub port: u16,
}

/// Where a provider stands with browser sign-in.
pub enum Availability {
    Ready(OAuthConfig),
    /// Supported, but needs an OAuth app you register (the message says how).
    NeedsSetup(String),
}

fn env(provider: &str, key: &str) -> Option<String> {
    std::env::var(format!(
        "ICE_OAUTH_{}_{key}",
        provider.to_ascii_uppercase().replace('-', "_")
    ))
    .ok()
    .filter(|s| !s.is_empty())
}

fn setting(provider: &str) -> Value {
    crate::settings::user_json()["oauth"][provider].clone()
}

/// Resolve the OAuth configuration for a provider (built-in + settings + env).
pub fn availability(provider: &str) -> Availability {
    let s = setting(provider);
    let get = |k: &str, e: &str| env(provider, e).or_else(|| s[k].as_str().map(String::from));
    let scopes_from = |default: &[&str]| -> Vec<String> {
        env(provider, "SCOPES")
            .map(|x| {
                x.split([' ', ','])
                    .filter(|p| !p.is_empty())
                    .map(String::from)
                    .collect()
            })
            .or_else(|| {
                s["scopes"].as_array().map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
            })
            .unwrap_or_else(|| default.iter().map(|x| x.to_string()).collect())
    };
    let port = get("port", "PORT")
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    if provider == "openrouter" {
        return Availability::Ready(OAuthConfig {
            authorize_url: get("authorizeUrl", "AUTHORIZE_URL")
                .unwrap_or_else(|| "https://openrouter.ai/auth".into()),
            token_url: get("tokenUrl", "TOKEN_URL")
                .unwrap_or_else(|| "https://openrouter.ai/api/v1/auth/keys".into()),
            client_id: String::new(),
            client_secret: None,
            scopes: vec![],
            exchange: Exchange::OpenRouterKey,
            port: if port == 0 { 3000 } else { port },
        });
    }
    let (default_auth, default_token, default_scopes): (Option<&str>, Option<&str>, &[&str]) =
        match provider {
            "gemini" => (
                Some("https://accounts.google.com/o/oauth2/v2/auth"),
                Some("https://oauth2.googleapis.com/token"),
                &[
                    "https://www.googleapis.com/auth/cloud-platform",
                    "https://www.googleapis.com/auth/generative-language.retriever",
                ],
            ),
            _ => (None, None, &["openid", "offline_access"]),
        };
    let authorize = get("authorizeUrl", "AUTHORIZE_URL").or(default_auth.map(String::from));
    let token = get("tokenUrl", "TOKEN_URL").or(default_token.map(String::from));
    let client = get("clientId", "CLIENT_ID");
    match (authorize, token, client) {
        (Some(a), Some(t), Some(c)) => Availability::Ready(OAuthConfig {
            authorize_url: a,
            token_url: t,
            client_id: c,
            client_secret: get("clientSecret", "CLIENT_SECRET"),
            scopes: scopes_from(default_scopes),
            exchange: Exchange::OAuth2,
            port,
        }),
        (a, t, c) => {
            let mut missing = Vec::new();
            if c.is_none() {
                missing.push("clientId");
            }
            if a.is_none() {
                missing.push("authorizeUrl");
            }
            if t.is_none() {
                missing.push("tokenUrl");
            }
            let hint = if provider == "gemini" {
                "Create an OAuth client (type: Desktop app) in Google Cloud Console → APIs & Services → Credentials, then run:\n  ice config set oauth '{\"gemini\":{\"clientId\":\"…\",\"clientSecret\":\"…\"}}'".to_string()
            } else {
                format!(
                    "This provider doesn't publish a public OAuth app for third-party tools. If your organisation fronts it with an OAuth/OIDC gateway, register ICE there (redirect: http://127.0.0.1/callback) and set:\n  ice config set oauth '{{\"{provider}\":{{\"authorizeUrl\":\"…\",\"tokenUrl\":\"…\",\"clientId\":\"…\"}}}}'\nOtherwise use an API key."
                )
            };
            Availability::NeedsSetup(format!(
                "Missing {} for {provider}.\n{hint}",
                missing.join(", ")
            ))
        }
    }
}

// ─────────────────────────── PKCE primitives ───────────────────────────

/// OS-seeded random bytes (std's RandomState draws its keys from the OS).
pub fn random_bytes(n: usize) -> Vec<u8> {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    #[cfg(unix)]
    {
        use std::io::Read;
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            let mut buf = vec![0u8; n];
            if f.read_exact(&mut buf).is_ok() {
                return buf;
            }
        }
    }
    let mut out = Vec::with_capacity(n);
    let mut i = 0u64;
    while out.len() < n {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u128(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        out.extend_from_slice(&h.finish().to_le_bytes());
        i += 1;
    }
    out.truncate(n);
    out
}

pub fn base64url(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut s = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        let chars = [(n >> 18) & 63, (n >> 12) & 63, (n >> 6) & 63, n & 63];
        for (i, c) in chars.iter().enumerate() {
            if i <= chunk.len() {
                s.push(A[*c as usize] as char);
            }
        }
    }
    s
}

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    pub fn new() -> Pkce {
        let verifier = base64url(&random_bytes(48)); // 64 chars, RFC 7636 §4.1
        Pkce::from_verifier(verifier)
    }

    pub fn from_verifier(verifier: String) -> Pkce {
        let challenge = base64url(&crate::update::sha256(verifier.as_bytes()));
        Pkce {
            verifier,
            challenge,
        }
    }
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| {
            let v = v.replace('+', " ");
            let bytes = v.as_bytes();
            let mut out = Vec::new();
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'%' && i + 2 < bytes.len() {
                    if let Ok(x) = u8::from_str_radix(&v[i + 1..i + 3], 16) {
                        out.push(x);
                        i += 3;
                        continue;
                    }
                }
                out.push(bytes[i]);
                i += 1;
            }
            String::from_utf8_lossy(&out).into_owned()
        })
    })
}

// ─────────────────────────── the flow ───────────────────────────

pub struct Pending {
    pub url: String,
    listener: TcpListener,
    redirect: String,
    pkce: Pkce,
    state: String,
    cfg: OAuthConfig,
}

/// Start a sign-in: bind the loopback callback and build the browser URL.
pub fn begin(cfg: OAuthConfig) -> Result<Pending> {
    let listener = TcpListener::bind(("127.0.0.1", cfg.port))
        .or_else(|_| TcpListener::bind(("127.0.0.1", 0)))
        .context("could not open a local port for the sign-in callback")?;
    let port = listener.local_addr()?.port();
    let redirect = format!("http://localhost:{port}/callback");
    let pkce = Pkce::new();
    let state = base64url(&random_bytes(16));
    let url = match cfg.exchange {
        Exchange::OpenRouterKey => format!(
            "{}?callback_url={}&code_challenge={}&code_challenge_method=S256",
            cfg.authorize_url,
            enc(&redirect),
            pkce.challenge
        ),
        Exchange::OAuth2 => {
            let sep = if cfg.authorize_url.contains('?') {
                '&'
            } else {
                '?'
            };
            let mut u = format!(
                "{}{sep}response_type=code&client_id={}&redirect_uri={}&code_challenge={}&code_challenge_method=S256&state={}",
                cfg.authorize_url,
                enc(&cfg.client_id),
                enc(&redirect),
                pkce.challenge,
                state
            );
            if !cfg.scopes.is_empty() {
                u.push_str(&format!("&scope={}", enc(&cfg.scopes.join(" "))));
            }
            if cfg.authorize_url.contains("accounts.google.com") {
                u.push_str("&access_type=offline&prompt=consent");
            }
            u
        }
    };
    Ok(Pending {
        url,
        listener,
        redirect,
        pkce,
        state,
        cfg,
    })
}

pub fn open_browser(url: &str) -> bool {
    let r = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).status()
    } else if cfg!(windows) {
        std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .status()
    } else {
        std::process::Command::new("xdg-open")
            .arg(url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
    };
    r.map(|s| s.success()).unwrap_or(false)
}

const PAGE_OK: &str = "<!doctype html><meta charset=utf-8><title>ICE</title><body style=\"font-family:system-ui;background:#061018;color:#e2f4fc;display:grid;place-items:center;height:100vh;margin:0\"><div><h2 style=\"color:#50d2ff\">■ ICE — signed in</h2><p>You can close this tab and return to your terminal.</p></div>";

/// Wait for the browser redirect (up to 5 minutes, cancellable), then
/// exchange the code. Returns what was stored: ("env var", "value") for key
/// flows or the provider's token record.
pub fn finish(p: Pending, cancel: &AtomicBool) -> Result<Tokens> {
    p.listener.set_nonblocking(true)?;
    let deadline = Instant::now() + Duration::from_secs(300);
    let (code, state) = loop {
        if cancel.load(Ordering::Relaxed) {
            bail!("sign-in cancelled");
        }
        if Instant::now() > deadline {
            bail!("sign-in timed out after 5 minutes");
        }
        match p.listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_nonblocking(false)?;
                stream.set_read_timeout(Some(Duration::from_secs(5)))?;
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line)?;
                let target = line.split_whitespace().nth(1).unwrap_or("");
                let query = target.split_once('?').map(|x| x.1).unwrap_or("");
                if !target.starts_with("/callback") {
                    let _ =
                        stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                    continue;
                }
                if let Some(err) = query_param(query, "error") {
                    let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\n\r\nSign-in failed. You can close this tab.");
                    bail!(
                        "the provider returned an error: {err} {}",
                        query_param(query, "error_description").unwrap_or_default()
                    );
                }
                let Some(code) = query_param(query, "code") else {
                    let _ =
                        stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n");
                    continue;
                };
                let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{PAGE_OK}", PAGE_OK.len());
                let _ = stream.write_all(resp.as_bytes());
                break (code, query_param(query, "state"));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(e) => return Err(e.into()),
        }
    };
    if p.cfg.exchange == Exchange::OAuth2 && state.as_deref() != Some(p.state.as_str()) {
        bail!("state mismatch in the sign-in callback — refusing the code (possible CSRF)");
    }
    exchange(&p.cfg, &code, &p.pkce.verifier, &p.redirect)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<u64>,
    /// True when the "access token" is a long-lived API key (OpenRouter).
    pub is_api_key: bool,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn exchange(cfg: &OAuthConfig, code: &str, verifier: &str, redirect: &str) -> Result<Tokens> {
    match cfg.exchange {
        Exchange::OpenRouterKey => {
            let body =
                json!({"code": code, "code_verifier": verifier, "code_challenge_method": "S256"});
            let resp = crate::http::post(
                &crate::http::Request {
                    url: &cfg.token_url,
                    headers: vec![],
                    body: &body,
                    max_retries: 2,
                },
                None,
                &|_, _, _| {},
            )?;
            let v: Value = serde_json::from_str(&resp.into_string()?)
                .context("invalid key-exchange response")?;
            let key = v["key"]
                .as_str()
                .ok_or_else(|| anyhow!("no key in the provider's response"))?;
            Ok(Tokens {
                access_token: key.into(),
                refresh_token: None,
                expires_at: None,
                is_api_key: true,
            })
        }
        Exchange::OAuth2 => {
            let mut form = vec![
                ("grant_type", "authorization_code".to_string()),
                ("code", code.to_string()),
                ("redirect_uri", redirect.to_string()),
                ("client_id", cfg.client_id.clone()),
                ("code_verifier", verifier.to_string()),
            ];
            if let Some(s) = &cfg.client_secret {
                form.push(("client_secret", s.clone()));
            }
            token_request(&cfg.token_url, &form)
        }
    }
}

fn token_request(url: &str, form: &[(&str, String)]) -> Result<Tokens> {
    let v = crate::http::post_form(url, form)?;
    let access = v["access_token"]
        .as_str()
        .ok_or_else(|| anyhow!("no access_token in the token response: {v}"))?;
    Ok(Tokens {
        access_token: access.into(),
        refresh_token: v["refresh_token"].as_str().map(String::from),
        expires_at: v["expires_in"].as_u64().map(|s| now_secs() + s),
        is_api_key: false,
    })
}

// ─────────────────────────── storage ───────────────────────────

/// Persist the result of a sign-in for `provider`.
pub fn store(provider: &str, t: &Tokens) -> Result<()> {
    if t.is_api_key {
        let env = crate::providers::find(provider)
            .map(|p| p.key_envs[0])
            .unwrap_or("ICE_API_KEY");
        return crate::settings::save_credential(env, &t.access_token);
    }
    crate::settings::save_credential_value(
        &format!("oauth:{provider}"),
        json!({"access_token": t.access_token, "refresh_token": t.refresh_token, "expires_at": t.expires_at}),
    )
}

/// A usable access token for `provider`, refreshing it when it's about to
/// expire. None when the provider was never signed in with OAuth.
pub fn access_token(provider: &str) -> Option<String> {
    let rec = crate::settings::credential_value(&format!("oauth:{provider}"))?;
    let token = rec["access_token"].as_str()?.to_string();
    let exp = rec["expires_at"].as_u64();
    if exp.map(|e| e > now_secs() + 60).unwrap_or(true) {
        return Some(token);
    }
    let refresh = rec["refresh_token"].as_str()?;
    let Availability::Ready(cfg) = availability(provider) else {
        return Some(token);
    };
    let mut form = vec![
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh.to_string()),
        ("client_id", cfg.client_id.clone()),
    ];
    if let Some(s) = &cfg.client_secret {
        form.push(("client_secret", s.clone()));
    }
    match token_request(&cfg.token_url, &form) {
        Ok(mut t) => {
            if t.refresh_token.is_none() {
                t.refresh_token = Some(refresh.to_string());
            }
            let _ = store(provider, &t);
            Some(t.access_token)
        }
        Err(_) => None,
    }
}

pub fn signed_in(provider: &str) -> bool {
    crate::settings::credential_value(&format!("oauth:{provider}")).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc7636_appendix_b_vector() {
        // The S256 example from RFC 7636, Appendix B.
        let p = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
        assert_eq!(p.challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn verifiers_are_long_random_and_urlsafe() {
        let a = Pkce::new();
        let b = Pkce::new();
        assert_ne!(a.verifier, b.verifier);
        assert!((43..=128).contains(&a.verifier.len()));
        assert!(a
            .verifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn builtin_and_configurable_providers() {
        assert!(
            matches!(availability("openrouter"), Availability::Ready(c) if c.exchange == Exchange::OpenRouterKey)
        );
        std::env::set_var(
            "ICE_OAUTH_GEMINI_CLIENT_ID",
            "abc.apps.googleusercontent.com",
        );
        assert!(
            matches!(availability("gemini"), Availability::Ready(c) if c.token_url.contains("googleapis"))
        );
        std::env::remove_var("ICE_OAUTH_GEMINI_CLIENT_ID");
        assert!(matches!(
            availability("mistral"),
            Availability::NeedsSetup(_)
        ));
    }

    #[test]
    fn full_pkce_flow_against_a_local_authorization_server() {
        // A tiny token endpoint that checks the verifier against the challenge.
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let token_url = format!(
            "http://127.0.0.1:{}/token",
            server.local_addr().unwrap().port()
        );
        let seen_challenge = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let cfg = OAuthConfig {
            authorize_url: "https://sso.example.com/authorize".into(),
            token_url: token_url.clone(),
            client_id: "ice".into(),
            client_secret: None,
            scopes: vec!["openid".into()],
            exchange: Exchange::OAuth2,
            port: 0,
        };
        let pending = begin(cfg).unwrap();
        let url = pending.url.clone();
        assert!(
            url.contains("code_challenge_method=S256")
                && url.contains("client_id=ice")
                && url.contains("scope=openid")
        );
        let challenge = query_param(url.split_once('?').unwrap().1, "code_challenge").unwrap();
        *seen_challenge.lock().unwrap() = challenge.clone();
        let state = query_param(url.split_once('?').unwrap().1, "state").unwrap();
        let redirect = query_param(url.split_once('?').unwrap().1, "redirect_uri").unwrap();
        let sc = seen_challenge.clone();
        std::thread::spawn(move || {
            let (mut s, _) = server.accept().unwrap();
            let mut buf = vec![0u8; 8192];
            let n = std::io::Read::read(&mut s, &mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let body = req.split("\r\n\r\n").nth(1).unwrap_or("");
            let verifier = query_param(body, "code_verifier").unwrap();
            let ok = Pkce::from_verifier(verifier).challenge == *sc.lock().unwrap()
                && query_param(body, "code").as_deref() == Some("abc");
            let reply = if ok {
                r#"{"access_token":"at-1","refresh_token":"rt-1","expires_in":3600}"#
            } else {
                r#"{"error":"invalid_grant"}"#
            };
            let _ = s.write_all(format!("HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{reply}", if ok { "200 OK" } else { "400 Bad Request" }, reply.len()).as_bytes());
        });
        // Simulate the browser redirect.
        let cb = format!("{redirect}?code=abc&state={state}").replace("localhost", "127.0.0.1");
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            let _ = crate::http::get_text(&cb, &[], Duration::from_secs(5), 4096);
        });
        let t = finish(pending, &AtomicBool::new(false)).unwrap();
        assert_eq!(t.access_token, "at-1");
        assert_eq!(t.refresh_token.as_deref(), Some("rt-1"));
        assert!(t.expires_at.unwrap() > now_secs());
    }
}
