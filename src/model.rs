//! Model adapters: stream one assistant turn from a provider.
//!
//! Two wire protocols cover every supported provider:
//! - Anthropic Messages (native tool_use, extended thinking, prompt caching)
//! - OpenAI Chat Completions (OpenAI, xAI, Groq, Gemini, OpenRouter, Ollama…)
//!
//! Models that reject native tools fall back to a text tool protocol, parsed
//! by `ir::parse_agent_step`, so every model can still drive the tools.

use crate::conv::{new_id, Block, Message, Role, Usage};
use crate::http::{self, HttpError, Request};
use crate::providers::{self, Provider};
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Api {
    Anthropic,
    OpenAi,
}

#[derive(Clone, Debug)]
pub struct ModelConfig {
    pub provider_id: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub api: Api,
    pub max_tokens: u32,
}

impl ModelConfig {
    /// Resolve provider, key, base URL and model from config + environment.
    /// `model_override` comes from `--model` / `/model`.
    pub fn resolve(model_override: Option<&str>) -> Option<Self> {
        let cfg = providers::IceConfig::load();
        let p: Option<&Provider> = providers::find(&cfg.provider);
        let key = p
            .and_then(providers::key_for)
            .or_else(|| std::env::var("ICE_API_KEY").ok().filter(|s| !s.is_empty()))
            .or_else(|| crate::oauth::access_token(&cfg.provider))
            .or_else(|| {
                // Local servers usually need no key.
                p.filter(|p| p.id == "ollama" || p.id == "custom")
                    .map(|_| "local".to_string())
            })?;
        let base = std::env::var("ICE_BASE_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .or(cfg.base_url.clone())
            .unwrap_or_else(|| p.map(|x| x.base_url.to_string()).unwrap_or_default());
        if base.is_empty() {
            return None;
        }
        let model = model_override
            .map(|m| m.to_string())
            .or_else(|| std::env::var("ICE_MODEL").ok().filter(|s| !s.is_empty()))
            .or_else(|| Some(cfg.model.clone()).filter(|s| !s.is_empty()))
            .unwrap_or_else(|| p.map(|x| x.default_model.to_string()).unwrap_or_default());
        let model = resolve_alias(&cfg.provider, &model);
        let api = if cfg.provider == "anthropic" || base.contains("anthropic.com") {
            Api::Anthropic
        } else {
            Api::OpenAi
        };
        let max_tokens = std::env::var("ICE_MAX_OUTPUT_TOKENS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| default_max_tokens(&model));
        Some(Self {
            provider_id: cfg.provider,
            base_url: base.trim_end_matches('/').to_string(),
            api_key: key,
            model,
            api,
            max_tokens,
        })
    }

    pub fn context_window(&self) -> u64 {
        context_window_for(&self.model)
    }
}

/// `sonnet`, `opus`, `haiku` shorthands (like Claude Code's `--model sonnet`).
pub fn resolve_alias(provider: &str, model: &str) -> String {
    if provider != "anthropic" {
        return model.to_string();
    }
    match model {
        "sonnet" => "claude-sonnet-4-5".into(),
        "opus" => "claude-opus-4-1".into(),
        "haiku" => "claude-haiku-4-5".into(),
        m => m.to_string(),
    }
}

fn default_max_tokens(model: &str) -> u32 {
    let m = model.to_ascii_lowercase();
    if m.contains("claude") {
        if m.contains("haiku-3") || m.contains("3-haiku") || m.contains("3-5") {
            8192
        } else {
            32_000
        }
    } else if m.contains("gpt-4o") || m.contains("llama") || m.contains("mistral") {
        16_384
    } else if m.contains("gemini") || m.contains("gpt-5") || m.contains("grok") {
        32_000
    } else {
        8192
    }
}

pub fn context_window_for(model: &str) -> u64 {
    let m = model.to_ascii_lowercase();
    if m.contains("[1m]") || m.contains("gemini") || m.contains("gpt-4.1") {
        1_000_000
    } else if m.contains("claude") {
        200_000
    } else if m.contains("gpt-5") {
        400_000
    } else if m.contains("grok-4") {
        256_000
    } else if m.contains("deepseek") {
        64_000
    } else if m.contains("llama3.1") || m.contains("llama-3") || m.contains("qwen") {
        131_072
    } else {
        128_000
    }
}

#[derive(Clone, Debug)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

pub struct TurnRequest<'a> {
    pub system: &'a str,
    pub messages: &'a [Message],
    pub tools: &'a [ToolSpec],
    /// Extended-thinking budget in tokens (0 = off).
    pub thinking_budget: u32,
    /// Send tools as text instructions instead of native tool definitions.
    pub text_tools: bool,
}

#[derive(Clone, Debug)]
pub enum StreamEvent {
    Text(String),
    Thinking(String),
    ToolStart,
    Retry {
        attempt: u32,
        error: String,
        wait: Duration,
    },
}

#[derive(Clone, Debug, Default)]
pub struct TurnResponse {
    pub content: Vec<Block>,
    pub stop_reason: String,
    pub usage: Usage,
}

/// Stream one assistant turn.
pub fn stream_turn(
    cfg: &ModelConfig,
    req: &TurnRequest,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(StreamEvent),
) -> Result<TurnResponse> {
    if let Some(mock) = mock_response(req) {
        return mock;
    }
    match cfg.api {
        Api::Anthropic => anthropic::stream(cfg, req, cancel, on),
        Api::OpenAi => openai::stream(cfg, req, cancel, on),
    }
}

/// True when the provider said it can't do native tool calling.
pub fn is_tools_unsupported(e: &anyhow::Error) -> bool {
    if let Some(h) = e.downcast_ref::<HttpError>() {
        if h.status == 400 || h.status == 404 || h.status == 422 || h.status == 500 {
            let b = h.body.to_ascii_lowercase();
            return (b.contains("tool") || b.contains("function"))
                && (b.contains("support")
                    || b.contains("not available")
                    || b.contains("unknown")
                    || b.contains("invalid"));
        }
    }
    false
}

type RetryFn<'a> = Box<dyn Fn(u32, &str, Duration) + 'a>;

fn retry_notifier<'a, 'b: 'a>(
    on: &'a std::cell::RefCell<&'b mut dyn FnMut(StreamEvent)>,
) -> RetryFn<'a> {
    Box::new(move |attempt, error, wait| {
        (on.borrow_mut())(StreamEvent::Retry {
            attempt,
            error: error.to_string(),
            wait,
        })
    })
}

/// Offline test hook: `ICE_MOCK_RESPONSES=/path/script.json` replays canned
/// turns (one JSON array of assistant content arrays, consumed in order).
/// Used by the integration tests; never set in normal use.
fn mock_response(_req: &TurnRequest) -> Option<Result<TurnResponse>> {
    let path = std::env::var("ICE_MOCK_RESPONSES").ok()?;
    use std::sync::Mutex;
    static CURSOR: Mutex<usize> = Mutex::new(0);
    let txt = std::fs::read_to_string(&path).ok()?;
    let all: Vec<Vec<Block>> = match serde_json::from_str(&txt) {
        Ok(v) => v,
        Err(e) => return Some(Err(anyhow!("bad mock script: {e}"))),
    };
    let mut c = CURSOR.lock().ok()?;
    let content = all.get(*c).cloned().unwrap_or_else(|| {
        vec![Block::Text {
            text: "(mock script exhausted)".into(),
        }]
    });
    *c += 1;
    let stop = if content.iter().any(|b| matches!(b, Block::ToolUse { .. })) {
        "tool_use"
    } else {
        "end_turn"
    };
    Some(Ok(TurnResponse {
        content,
        stop_reason: stop.into(),
        usage: Usage {
            input_tokens: 100,
            output_tokens: 20,
            ..Default::default()
        },
    }))
}

// ───────────────────────────── Anthropic ─────────────────────────────

mod anthropic {
    use super::*;

    pub fn build_body(cfg: &ModelConfig, req: &TurnRequest) -> Value {
        let mut messages: Vec<Value> = req
            .messages
            .iter()
            .map(|m| {
                let content: Vec<Value> = m
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        // Thinking blocks are only valid when echoed back with
                        // their signature; drop unsigned ones.
                        Block::Thinking { signature, .. } if signature.is_empty() => None,
                        other => serde_json::to_value(other).ok(),
                    })
                    .collect();
                json!({"role": m.role, "content": content})
            })
            .filter(|m| {
                m["content"]
                    .as_array()
                    .map(|a| !a.is_empty())
                    .unwrap_or(false)
            })
            .collect();
        // Prompt caching: mark the newest user turn so the whole prefix is
        // cached for the next request (the second-newest stays cached too).
        let mut marked = 0;
        for m in messages.iter_mut().rev() {
            if marked == 2 {
                break;
            }
            if m["role"] == "user" {
                if let Some(last) = m["content"].as_array_mut().and_then(|a| a.last_mut()) {
                    last["cache_control"] = json!({"type": "ephemeral"});
                    marked += 1;
                }
            }
        }
        let mut body = json!({
            "model": cfg.model,
            "max_tokens": cfg.max_tokens,
            "stream": true,
            "system": [{"type": "text", "text": req.system, "cache_control": {"type": "ephemeral"}}],
            "messages": messages,
        });
        if !req.tools.is_empty() && !req.text_tools {
            let mut tools: Vec<Value> = req
                .tools
                .iter()
                .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.input_schema}))
                .collect();
            if let Some(last) = tools.last_mut() {
                last["cache_control"] = json!({"type": "ephemeral"});
            }
            body["tools"] = Value::Array(tools);
        }
        if req.thinking_budget > 0 {
            let budget = req
                .thinking_budget
                .min(cfg.max_tokens.saturating_sub(1024))
                .max(1024);
            body["thinking"] = json!({"type": "enabled", "budget_tokens": budget});
        }
        body
    }

    pub fn stream(
        cfg: &ModelConfig,
        req: &TurnRequest,
        cancel: &AtomicBool,
        on: &mut dyn FnMut(StreamEvent),
    ) -> Result<TurnResponse> {
        let body = build_body(cfg, req);
        let url = format!("{}/messages", cfg.base_url);
        let mut headers = vec![("anthropic-version".to_string(), "2023-06-01".to_string())];
        if cfg.api_key.starts_with("sk-ant-oat") {
            headers.push(("authorization".into(), format!("Bearer {}", cfg.api_key)));
            headers.push(("anthropic-beta".into(), "oauth-2025-04-20".into()));
        } else {
            headers.push(("x-api-key".into(), cfg.api_key.clone()));
        }
        let on = std::cell::RefCell::new(on);
        let resp = http::post(
            &Request {
                url: &url,
                headers,
                body: &body,
                max_retries: 8,
            },
            Some(cancel),
            &retry_notifier(&on),
        )?;
        let on = on.into_inner();

        let mut blocks: BTreeMap<usize, Block> = BTreeMap::new();
        let mut json_bufs: BTreeMap<usize, String> = BTreeMap::new();
        let mut out = TurnResponse::default();
        http::read_sse(resp, Some(cancel), |event, data| {
            let v: Value = serde_json::from_str(data).unwrap_or(Value::Null);
            let kind = v["type"].as_str().unwrap_or(event);
            match kind {
                "message_start" => {
                    merge_usage(&mut out.usage, &v["message"]["usage"]);
                }
                "content_block_start" => {
                    let idx = v["index"].as_u64().unwrap_or(0) as usize;
                    let cb = &v["content_block"];
                    let block = match cb["type"].as_str().unwrap_or("") {
                        "tool_use" => {
                            let name = cb["name"].as_str().unwrap_or("").to_string();
                            on(StreamEvent::ToolStart);
                            Block::ToolUse {
                                id: cb["id"].as_str().unwrap_or("").to_string(),
                                name,
                                input: json!({}),
                            }
                        }
                        "thinking" => Block::Thinking {
                            thinking: String::new(),
                            signature: String::new(),
                        },
                        "redacted_thinking" => Block::RedactedThinking {
                            data: cb["data"].as_str().unwrap_or("").to_string(),
                        },
                        _ => Block::Text {
                            text: String::new(),
                        },
                    };
                    blocks.insert(idx, block);
                }
                "content_block_delta" => {
                    let idx = v["index"].as_u64().unwrap_or(0) as usize;
                    let d = &v["delta"];
                    match (d["type"].as_str().unwrap_or(""), blocks.get_mut(&idx)) {
                        ("text_delta", Some(Block::Text { text })) => {
                            let t = d["text"].as_str().unwrap_or("");
                            text.push_str(t);
                            on(StreamEvent::Text(t.to_string()));
                        }
                        ("thinking_delta", Some(Block::Thinking { thinking, .. })) => {
                            let t = d["thinking"].as_str().unwrap_or("");
                            thinking.push_str(t);
                            on(StreamEvent::Thinking(t.to_string()));
                        }
                        ("signature_delta", Some(Block::Thinking { signature, .. })) => {
                            signature.push_str(d["signature"].as_str().unwrap_or(""));
                        }
                        ("input_json_delta", Some(Block::ToolUse { .. })) => {
                            json_bufs
                                .entry(idx)
                                .or_default()
                                .push_str(d["partial_json"].as_str().unwrap_or(""));
                        }
                        _ => {}
                    }
                }
                "content_block_stop" => {
                    let idx = v["index"].as_u64().unwrap_or(0) as usize;
                    if let (Some(Block::ToolUse { input, .. }), Some(buf)) =
                        (blocks.get_mut(&idx), json_bufs.remove(&idx))
                    {
                        *input = if buf.trim().is_empty() {
                            json!({})
                        } else {
                            serde_json::from_str(&buf).unwrap_or_else(|_| json!({"_raw": buf}))
                        };
                    }
                }
                "message_delta" => {
                    if let Some(s) = v["delta"]["stop_reason"].as_str() {
                        out.stop_reason = s.to_string();
                    }
                    merge_usage(&mut out.usage, &v["usage"]);
                }
                "error" => {
                    let msg = v["error"]["message"].as_str().unwrap_or(data);
                    let typ = v["error"]["type"].as_str().unwrap_or("error");
                    let status = if typ == "overloaded_error" { 529 } else { 500 };
                    return Err(HttpError {
                        status,
                        body: json!({"error": {"message": msg}}).to_string(),
                    }
                    .into());
                }
                _ => {}
            }
            Ok(())
        })?;
        out.content = blocks
            .into_values()
            .filter(|b| !matches!(b, Block::Text { text } if text.is_empty()))
            .collect();
        if req.text_tools {
            out.content = super::text_protocol::extract(out.content);
        }
        Ok(out)
    }

    fn merge_usage(u: &mut Usage, v: &Value) {
        let get = |k: &str| v.get(k).and_then(|x| x.as_u64());
        if let Some(n) = get("input_tokens") {
            u.input_tokens = n;
        }
        if let Some(n) = get("output_tokens") {
            u.output_tokens = n;
        }
        if let Some(n) = get("cache_read_input_tokens") {
            u.cache_read_input_tokens = n;
        }
        if let Some(n) = get("cache_creation_input_tokens") {
            u.cache_creation_input_tokens = n;
        }
    }
}

// ───────────────────────────── OpenAI-compatible ─────────────────────────────

mod openai {
    use super::*;

    /// Flags that let a request degrade gracefully across compatible servers.
    #[derive(Clone, Copy, Default)]
    pub struct Compat {
        pub no_stream_options: bool,
        pub max_completion_tokens: bool,
    }

    pub fn build_body(cfg: &ModelConfig, req: &TurnRequest, compat: Compat) -> Value {
        let mut messages = vec![json!({"role": "system", "content": req.system})];
        for m in req.messages {
            match m.role {
                Role::User => {
                    let mut text = String::new();
                    let mut results = Vec::new();
                    for b in &m.content {
                        match b {
                            Block::Text { text: t } => {
                                if !text.is_empty() {
                                    text.push('\n');
                                }
                                text.push_str(t);
                            }
                            Block::ToolResult {
                                tool_use_id,
                                content,
                                is_error,
                            } => results.push((tool_use_id.clone(), content.clone(), *is_error)),
                            _ => {}
                        }
                    }
                    if req.text_tools {
                        let mut s = String::new();
                        for (_, c, err) in &results {
                            s.push_str(&format!(
                                "<tool_result{}>\n{c}\n</tool_result>\n",
                                if *err { " error=\"true\"" } else { "" }
                            ));
                        }
                        s.push_str(&text);
                        if !s.trim().is_empty() {
                            messages.push(json!({"role": "user", "content": s.trim()}));
                        }
                    } else {
                        for (id, c, _) in results {
                            messages
                                .push(json!({"role": "tool", "tool_call_id": id, "content": c}));
                        }
                        if !text.is_empty() {
                            messages.push(json!({"role": "user", "content": text}));
                        }
                    }
                }
                Role::Assistant => {
                    let mut text = String::new();
                    let mut calls = Vec::new();
                    for b in &m.content {
                        match b {
                            Block::Text { text: t } => text.push_str(t),
                            Block::ToolUse { id, name, input } => {
                                if req.text_tools {
                                    text.push('\n');
                                    text.push_str(&super::text_protocol::render_call(name, input));
                                } else {
                                    calls.push(json!({
                                        "id": id,
                                        "type": "function",
                                        "function": {"name": name, "arguments": input.to_string()}
                                    }));
                                }
                            }
                            _ => {}
                        }
                    }
                    let mut msg = json!({"role": "assistant"});
                    msg["content"] = if text.is_empty() && !calls.is_empty() {
                        Value::Null
                    } else {
                        Value::String(text)
                    };
                    if !calls.is_empty() {
                        msg["tool_calls"] = Value::Array(calls);
                    }
                    messages.push(msg);
                }
            }
        }
        let mut body = json!({
            "model": cfg.model,
            "stream": true,
            "messages": messages,
        });
        if compat.max_completion_tokens {
            body["max_completion_tokens"] = json!(cfg.max_tokens);
        } else {
            body["max_tokens"] = json!(cfg.max_tokens);
        }
        if !compat.no_stream_options {
            body["stream_options"] = json!({"include_usage": true});
        }
        if !req.tools.is_empty() && !req.text_tools {
            body["tools"] = Value::Array(
                req.tools
                    .iter()
                    .map(|t| {
                        json!({"type": "function", "function": {
                            "name": t.name, "description": t.description, "parameters": t.input_schema
                        }})
                    })
                    .collect(),
            );
        }
        if req.thinking_budget > 0 && (cfg.model.starts_with("o") || cfg.model.contains("gpt-5")) {
            body["reasoning_effort"] = json!(if req.thinking_budget > 16_000 {
                "high"
            } else {
                "medium"
            });
        }
        body
    }

    pub fn stream(
        cfg: &ModelConfig,
        req: &TurnRequest,
        cancel: &AtomicBool,
        on: &mut dyn FnMut(StreamEvent),
    ) -> Result<TurnResponse> {
        let url = format!("{}/chat/completions", cfg.base_url);
        let mut compat = Compat::default();
        let on = std::cell::RefCell::new(on);
        // Up to two downgrades for servers that reject optional parameters.
        let resp = loop {
            let body = build_body(cfg, req, compat);
            let headers = vec![(
                "authorization".to_string(),
                format!("Bearer {}", cfg.api_key),
            )];
            match http::post(
                &Request {
                    url: &url,
                    headers,
                    body: &body,
                    max_retries: 8,
                },
                Some(cancel),
                &retry_notifier(&on),
            ) {
                Ok(r) => break r,
                Err(e) => {
                    if let Some(h) = e.downcast_ref::<HttpError>() {
                        let b = h.body.to_ascii_lowercase();
                        if (h.status == 400 || h.status == 422)
                            && b.contains("stream_options")
                            && !compat.no_stream_options
                        {
                            compat.no_stream_options = true;
                            continue;
                        }
                        if h.status == 400
                            && b.contains("max_completion_tokens")
                            && !compat.max_completion_tokens
                        {
                            compat.max_completion_tokens = true;
                            continue;
                        }
                    }
                    return Err(e);
                }
            }
        };
        let on = on.into_inner();

        let mut text = String::new();
        let mut thinking = String::new();
        // index → (id, name, arguments)
        let mut calls: BTreeMap<usize, (String, String, String)> = BTreeMap::new();
        let mut out = TurnResponse::default();
        let mut in_think_tag = false;
        http::read_sse(resp, Some(cancel), |_event, data| {
            if data.trim() == "[DONE]" {
                return Ok(());
            }
            let v: Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => return Ok(()),
            };
            if let Some(err) = v.get("error") {
                bail!(HttpError {
                    status: 500,
                    body: json!({"error": err}).to_string()
                });
            }
            if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
                let cached = u["prompt_tokens_details"]["cached_tokens"]
                    .as_u64()
                    .unwrap_or(0);
                out.usage.input_tokens = u["prompt_tokens"]
                    .as_u64()
                    .unwrap_or(0)
                    .saturating_sub(cached);
                out.usage.cache_read_input_tokens = cached;
                out.usage.output_tokens = u["completion_tokens"].as_u64().unwrap_or(0);
            }
            let Some(choice) = v["choices"].get(0) else {
                return Ok(());
            };
            let d = &choice["delta"];
            for key in ["reasoning_content", "reasoning"] {
                if let Some(t) = d[key].as_str() {
                    if !t.is_empty() {
                        thinking.push_str(t);
                        on(StreamEvent::Thinking(t.to_string()));
                    }
                }
            }
            if let Some(t) = d["content"].as_str() {
                // Some open models inline <think>…</think> in the content.
                let mut rest = t;
                while !rest.is_empty() {
                    if in_think_tag {
                        if let Some(end) = rest.find("</think>") {
                            thinking.push_str(&rest[..end]);
                            on(StreamEvent::Thinking(rest[..end].to_string()));
                            rest = &rest[end + 8..];
                            in_think_tag = false;
                        } else {
                            thinking.push_str(rest);
                            on(StreamEvent::Thinking(rest.to_string()));
                            rest = "";
                        }
                    } else if let Some(start) = rest.find("<think>") {
                        let before = &rest[..start];
                        if !before.is_empty() {
                            text.push_str(before);
                            on(StreamEvent::Text(before.to_string()));
                        }
                        rest = &rest[start + 7..];
                        in_think_tag = true;
                    } else {
                        text.push_str(rest);
                        on(StreamEvent::Text(rest.to_string()));
                        rest = "";
                    }
                }
            }
            if let Some(tcs) = d["tool_calls"].as_array() {
                for (n, tc) in tcs.iter().enumerate() {
                    let idx = tc["index"].as_u64().map(|i| i as usize).unwrap_or(n);
                    let entry = calls.entry(idx).or_default();
                    if let Some(id) = tc["id"].as_str() {
                        if !id.is_empty() {
                            entry.0 = id.to_string();
                        }
                    }
                    if let Some(name) = tc["function"]["name"].as_str() {
                        if !name.is_empty() && entry.1.is_empty() {
                            entry.1 = name.to_string();
                            on(StreamEvent::ToolStart);
                        }
                    }
                    match &tc["function"]["arguments"] {
                        Value::String(s) => entry.2.push_str(s),
                        Value::Object(_) => entry.2 = tc["function"]["arguments"].to_string(),
                        _ => {}
                    }
                }
            }
            if let Some(fr) = choice["finish_reason"].as_str() {
                out.stop_reason = match fr {
                    "tool_calls" | "function_call" => "tool_use".into(),
                    "length" => "max_tokens".into(),
                    _ => "end_turn".into(),
                };
            }
            Ok(())
        })?;

        if !thinking.trim().is_empty() {
            out.content.push(Block::Thinking {
                thinking,
                signature: String::new(),
            });
        }
        if !text.trim().is_empty() {
            out.content.push(Block::Text { text });
        }
        for (_, (id, name, args)) in calls {
            if name.is_empty() {
                continue;
            }
            let input = if args.trim().is_empty() {
                json!({})
            } else {
                serde_json::from_str(&args).unwrap_or_else(|_| json!({"_raw": args}))
            };
            out.content.push(Block::ToolUse {
                id: if id.is_empty() { new_id("call") } else { id },
                name,
                input,
            });
        }
        if out
            .content
            .iter()
            .any(|b| matches!(b, Block::ToolUse { .. }))
        {
            out.stop_reason = "tool_use".into();
        }
        if req.text_tools {
            out.content = super::text_protocol::extract(out.content);
        }
        Ok(out)
    }
}

// ───────────────────────────── Text tool protocol ─────────────────────────────

pub mod text_protocol {
    use super::*;
    use crate::ir::{self, Action};

    /// Instructions appended to the system prompt when a model can't do
    /// native tool calls.
    pub fn instructions(tools: &[ToolSpec]) -> String {
        let mut s = String::from(
            "\n\n# Calling tools\nThis model has no native tool calling. To call a tool, write an XML block exactly like:\n\
<function=TOOL_NAME>\n<parameter=PARAM_NAME>value</parameter>\n</function>\n\
You may call several tools in one reply. After your calls, STOP and wait: the results arrive in the next message inside <tool_result> tags. \
When the task is complete, reply with plain text and no tool call.\n\nAvailable tools:\n",
        );
        for t in tools {
            let params: Vec<String> = t.input_schema["properties"]
                .as_object()
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default();
            let first = t.description.lines().next().unwrap_or("");
            s.push_str(&format!("- {}({}): {}\n", t.name, params.join(", "), first));
        }
        s
    }

    pub fn render_call(name: &str, input: &Value) -> String {
        let mut s = format!("<function={name}>\n");
        if let Some(o) = input.as_object() {
            for (k, v) in o {
                let val = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                s.push_str(&format!("<parameter={k}>{val}</parameter>\n"));
            }
        }
        s.push_str("</function>");
        s
    }

    /// Turn text-protocol tool calls in the assistant text into ToolUse blocks.
    pub fn extract(content: Vec<Block>) -> Vec<Block> {
        let mut out = Vec::new();
        for b in content {
            match b {
                Block::Text { text } => {
                    let generic = parse_generic_calls(&text);
                    if !generic.is_empty() {
                        let prose = strip_calls(&text);
                        if !prose.trim().is_empty() {
                            out.push(Block::Text { text: prose });
                        }
                        out.extend(generic);
                        continue;
                    }
                    let step = ir::parse_agent_step(&text);
                    if step.actions.is_empty() {
                        out.push(Block::Text { text });
                        continue;
                    }
                    if !step.text.trim().is_empty() {
                        out.push(Block::Text {
                            text: step.text.clone(),
                        });
                    }
                    for a in step.actions {
                        if let Some((name, input)) = action_to_tool(a) {
                            out.push(Block::ToolUse {
                                id: new_id("toolu"),
                                name,
                                input,
                            });
                        }
                    }
                }
                other => out.push(other),
            }
        }
        out
    }

    /// `<function=Name><parameter=k>v</parameter></function>` with the real
    /// tool names — kept verbatim so every parameter survives.
    fn parse_generic_calls(text: &str) -> Vec<Block> {
        let fn_re =
            regex::Regex::new(r"(?s)<function=([A-Za-z0-9_\-]+)\s*>(.*?)</function>").unwrap();
        let p_re =
            regex::Regex::new(r"(?s)<parameter=([A-Za-z0-9_\-]+)\s*>(.*?)</parameter>").unwrap();
        let mut out = Vec::new();
        for cap in fn_re.captures_iter(text) {
            let raw_name = &cap[1];
            let Some(name) = crate::tools::canonical_name(raw_name) else {
                continue;
            };
            let mut input = Map::new();
            for p in p_re.captures_iter(&cap[2]) {
                let v = p[2].strip_prefix('\n').unwrap_or(&p[2]);
                let v = v.strip_suffix('\n').unwrap_or(v);
                let val = match v.trim() {
                    "true" => Value::Bool(true),
                    "false" => Value::Bool(false),
                    t if t.starts_with('[') || t.starts_with('{') => {
                        serde_json::from_str(t).unwrap_or_else(|_| Value::String(v.to_string()))
                    }
                    t if !t.is_empty() && t.len() < 12 && t.chars().all(|c| c.is_ascii_digit()) => {
                        json!(t.parse::<u64>().unwrap_or(0))
                    }
                    _ => Value::String(v.to_string()),
                };
                input.insert(p[1].to_string(), val);
            }
            out.push(Block::ToolUse {
                id: new_id("toolu"),
                name,
                input: Value::Object(input),
            });
        }
        out
    }

    fn strip_calls(text: &str) -> String {
        let re =
            regex::Regex::new(r"(?s)<tool_call>|</tool_call>|<function=.*?</function>").unwrap();
        re.replace_all(text, "").trim().to_string()
    }

    fn action_to_tool(a: Action) -> Option<(String, Value)> {
        Some(match a {
            Action::Read { path } => ("Read".into(), json!({"file_path": path})),
            Action::List { path } => ("LS".into(), json!({"path": path})),
            Action::Grep { pattern, path } => {
                ("Grep".into(), json!({"pattern": pattern, "path": path}))
            }
            Action::Write { path, contents } => (
                "Write".into(),
                json!({"file_path": path, "content": contents}),
            ),
            Action::Replace { path, old, new } => (
                "Edit".into(),
                json!({"file_path": path, "old_string": old, "new_string": new}),
            ),
            Action::Run { cmd } => ("Bash".into(), json!({"command": cmd})),
            Action::WebSearch { query } => ("WebSearch".into(), json!({"query": query})),
            Action::WebFetch { url } => ("WebFetch".into(), json!({"url": url, "prompt": ""})),
            Action::Agent { name, goal } => (
                "Task".into(),
                json!({"subagent_type": name, "description": goal.chars().take(40).collect::<String>(), "prompt": goal}),
            ),
            Action::Mcp { tool, args } => {
                let (server, t) = tool.split_once('/').unwrap_or(("", tool.as_str()));
                (
                    format!("mcp__{server}__{t}"),
                    serde_json::from_str(&args).unwrap_or_else(|_| json!({})),
                )
            }
            Action::Skill { name } => ("Skill".into(), json!({"name": name})),
        })
    }
}

/// One-shot, tool-less completion (used for /compact summaries, WebFetch
/// post-processing and session titles).
pub fn complete_text(
    cfg: &ModelConfig,
    system: &str,
    prompt: &str,
    cancel: &AtomicBool,
) -> Result<(String, Usage)> {
    let msgs = [Message::user_text(prompt)];
    let r = stream_turn(
        cfg,
        &TurnRequest {
            system,
            messages: &msgs,
            tools: &[],
            thinking_budget: 0,
            text_tools: false,
        },
        cancel,
        &mut |_| {},
    )?;
    let text: String = r
        .content
        .iter()
        .filter_map(|b| match b {
            Block::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    if text.trim().is_empty() {
        bail!("model returned an empty response");
    }
    Ok((text, r.usage))
}

/// Published list prices (USD per million input/output tokens) for common
/// models. Unknown models return None and costs are shown as tokens only.
pub fn price(model: &str) -> Option<(f64, f64)> {
    let m = model.to_ascii_lowercase();
    let table: &[(&str, f64, f64)] = &[
        ("claude-opus-4-5", 5.0, 25.0),
        ("claude-opus-4", 15.0, 75.0),
        ("claude-3-opus", 15.0, 75.0),
        ("claude-sonnet-4", 3.0, 15.0),
        ("claude-3-7-sonnet", 3.0, 15.0),
        ("claude-3-5-sonnet", 3.0, 15.0),
        ("claude-haiku-4-5", 1.0, 5.0),
        ("claude-3-5-haiku", 0.8, 4.0),
        ("gpt-4o-mini", 0.15, 0.6),
        ("gpt-4o", 2.5, 10.0),
        ("gpt-4.1-nano", 0.1, 0.4),
        ("gpt-4.1-mini", 0.4, 1.6),
        ("gpt-4.1", 2.0, 8.0),
        ("gpt-5-nano", 0.05, 0.4),
        ("gpt-5-mini", 0.25, 2.0),
        ("gpt-5", 1.25, 10.0),
        ("gemini-2.5-pro", 1.25, 10.0),
        ("gemini-2.5-flash-lite", 0.1, 0.4),
        ("gemini-2.5-flash", 0.3, 2.5),
    ];
    table
        .iter()
        .find(|(k, _, _)| m.contains(k))
        .map(|(_, i, o)| (*i, *o))
}

/// Estimated USD cost of a usage record (cache reads at 10%, cache writes
/// at 125% of the input price).
pub fn cost(model: &str, u: &Usage) -> Option<f64> {
    let (i, o) = price(model)?;
    Some(
        (u.input_tokens as f64 * i
            + u.cache_read_input_tokens as f64 * i * 0.1
            + u.cache_creation_input_tokens as f64 * i * 1.25
            + u.output_tokens as f64 * o)
            / 1_000_000.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(api: Api) -> ModelConfig {
        ModelConfig {
            provider_id: "x".into(),
            base_url: "http://localhost".into(),
            api_key: "k".into(),
            model: "m".into(),
            api,
            max_tokens: 1000,
        }
    }

    fn convo() -> Vec<Message> {
        vec![
            Message::user_text("list files"),
            Message {
                role: Role::Assistant,
                content: vec![
                    Block::Text {
                        text: "Looking.".into(),
                    },
                    Block::ToolUse {
                        id: "c1".into(),
                        name: "LS".into(),
                        input: json!({"path": "."}),
                    },
                ],
            },
            Message {
                role: Role::User,
                content: vec![Block::ToolResult {
                    tool_use_id: "c1".into(),
                    content: "a.rs".into(),
                    is_error: false,
                }],
            },
        ]
    }

    #[test]
    fn openai_body_threads_tool_calls_and_results() {
        let msgs = convo();
        let tools = vec![ToolSpec {
            name: "LS".into(),
            description: "list".into(),
            input_schema: json!({"type":"object"}),
        }];
        let req = TurnRequest {
            system: "sys",
            messages: &msgs,
            tools: &tools,
            thinking_budget: 0,
            text_tools: false,
        };
        let b = openai::build_body(&cfg(Api::OpenAi), &req, openai::Compat::default());
        let m = b["messages"].as_array().unwrap();
        assert_eq!(m[0]["role"], "system");
        assert_eq!(m[2]["tool_calls"][0]["function"]["name"], "LS");
        assert_eq!(m[3]["role"], "tool");
        assert_eq!(m[3]["tool_call_id"], "c1");
        assert_eq!(b["tools"][0]["function"]["name"], "LS");
        assert_eq!(b["stream_options"]["include_usage"], true);
    }

    #[test]
    fn anthropic_body_caches_prefix_and_tools() {
        let msgs = convo();
        let tools = vec![ToolSpec {
            name: "LS".into(),
            description: "list".into(),
            input_schema: json!({"type":"object"}),
        }];
        let req = TurnRequest {
            system: "sys",
            messages: &msgs,
            tools: &tools,
            thinking_budget: 0,
            text_tools: false,
        };
        let b = anthropic::build_body(&cfg(Api::Anthropic), &req);
        assert_eq!(b["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(b["tools"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(b["messages"][2]["content"][0]["type"], "tool_result");
        assert_eq!(
            b["messages"][2]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
    }

    #[test]
    fn text_protocol_extracts_xml_and_plain_calls() {
        let blocks = text_protocol::extract(vec![Block::Text {
            text: "Reading it.\n<function=Read>\n<parameter=file_path>src/main.rs</parameter>\n<parameter=limit>20</parameter>\n</function>".into(),
        }]);
        assert_eq!(blocks.len(), 2);
        match &blocks[1] {
            Block::ToolUse { name, input, .. } => {
                assert_eq!(name, "Read");
                assert_eq!(input["file_path"], "src/main.rs");
                assert_eq!(input["limit"], 20);
            }
            _ => panic!("expected tool use"),
        }
        let plain = text_protocol::extract(vec![Block::Text {
            text: "run cargo test".into(),
        }]);
        assert!(matches!(&plain[0], Block::ToolUse { name, .. } if name == "Bash"));
    }

    #[test]
    fn aliases_resolve_only_for_anthropic() {
        assert_eq!(resolve_alias("anthropic", "sonnet"), "claude-sonnet-4-5");
        assert_eq!(resolve_alias("openai", "sonnet"), "sonnet");
    }
}
