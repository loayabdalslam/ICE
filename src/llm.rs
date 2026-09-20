use crate::providers::{self, Provider};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::io::Write;
use std::process::Command;

pub struct Completion {
    pub content: String,
    pub thinking: String,
}

pub struct Llm {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub provider_id: String,
}

impl Llm {
    pub fn from_env() -> Option<Self> {
        let cfg = providers::IceConfig::load();
        let p = providers::find(&cfg.provider).copied();
        let p_ref: Option<&Provider> = p.as_ref();
        let key = p_ref
            .and_then(providers::key_for)
            .or_else(|| std::env::var("ICE_API_KEY").ok().filter(|s| !s.is_empty()))
            .or_else(|| std::env::var("XAI_API_KEY").ok().filter(|s| !s.is_empty()))
            .or_else(|| {
                std::env::var("OPENAI_API_KEY")
                    .ok()
                    .filter(|s| !s.is_empty())
            })
            .or_else(|| std::env::var("GROQ_API_KEY").ok().filter(|s| !s.is_empty()))?;
        let base = cfg
            .base_url
            .or_else(|| std::env::var("ICE_BASE_URL").ok())
            .or_else(|| std::env::var("OPENAI_BASE_URL").ok())
            .unwrap_or_else(|| {
                p_ref
                    .map(|x| x.base_url.to_string())
                    .unwrap_or_else(|| "https://api.x.ai/v1".into())
            });
        let model = if cfg.model.is_empty() {
            std::env::var("ICE_MODEL").unwrap_or_else(|_| {
                p_ref
                    .map(|x| x.default_model.to_string())
                    .unwrap_or_else(|| "grok-3".into())
            })
        } else {
            cfg.model
        };
        Some(Self {
            base_url: base.trim_end_matches('/').to_string(),
            api_key: key,
            model,
            provider_id: cfg.provider,
        })
    }

    pub fn complete(&self, system: &str, user: &str) -> Result<Completion> {
        self.complete_messages(system, &[("user".into(), user.into())])
    }

    /// Multi-turn completion — the basis of the agentic loop. Roles are
    /// "user"/"assistant"; the system prompt is passed separately.
    pub fn complete_messages(&self, system: &str, msgs: &[(String, String)]) -> Result<Completion> {
        if self.provider_id == "anthropic" || self.base_url.contains("anthropic.com") {
            return self.complete_anthropic(system, msgs);
        }
        let url = format!("{}/chat/completions", self.base_url);
        let mut messages = vec![json!({"role": "system", "content": system})];
        for (role, content) in msgs {
            messages.push(json!({"role": role, "content": content}));
        }
        let body = json!({
            "model": self.model,
            "temperature": 0.2,
            "max_tokens": 4096,
            "messages": messages
        });
        let raw = curl_json(&url, &self.api_key, &body, &[])?;
        if let Some(err) = raw.get("error") {
            bail!("llm error: {err}");
        }
        let msg = &raw["choices"][0]["message"];
        let mut content = msg["content"].as_str().unwrap_or("").to_string();
        // Native OpenAI tool calls arrive in message.tool_calls, not content.
        // Normalize them into markup the burst parser understands.
        if let Some(calls) = msg.get("tool_calls").and_then(|x| x.as_array()) {
            for c in calls {
                let f = c.get("function").unwrap_or(c);
                let name = f.get("name").and_then(|x| x.as_str()).unwrap_or("");
                let args = f
                    .get("arguments")
                    .map(|a| match a {
                        Value::String(s) => {
                            serde_json::from_str::<Value>(s).unwrap_or(Value::Null)
                        }
                        other => other.clone(),
                    })
                    .unwrap_or(Value::Null);
                if !name.is_empty() {
                    content.push('\n');
                    content.push_str(&tool_call_markup(name, &args));
                }
            }
        }
        let thinking = msg
            .get("reasoning_content")
            .and_then(|x| x.as_str())
            .or_else(|| msg.get("reasoning").and_then(|x| x.as_str()))
            .unwrap_or("")
            .to_string();
        Ok(split_think(content, thinking))
    }

    fn complete_anthropic(&self, system: &str, msgs: &[(String, String)]) -> Result<Completion> {
        let url = format!("{}/messages", self.base_url.trim_end_matches('/'));
        let messages: Vec<Value> = msgs
            .iter()
            .map(|(role, content)| json!({"role": role, "content": content}))
            .collect();
        let body = json!({
            "model": self.model,
            "max_tokens": 4096,
            "system": system,
            "messages": messages
        });
        let extra = [
            "anthropic-version: 2023-06-01".to_string(),
            format!("x-api-key: {}", self.api_key),
        ];
        let raw = curl_json(&url, &self.api_key, &body, &extra)?;
        if let Some(err) = raw.get("error") {
            bail!("llm error: {err}");
        }
        let mut content = String::new();
        let mut thinking = String::new();
        if let Some(arr) = raw.get("content").and_then(|c| c.as_array()) {
            for block in arr {
                let kind = block.get("type").and_then(|x| x.as_str()).unwrap_or("");
                match kind {
                    "thinking" => {
                        thinking.push_str(block.get("text").and_then(|x| x.as_str()).unwrap_or(""))
                    }
                    // Anthropic native tool call block.
                    "tool_use" => {
                        let name = block.get("name").and_then(|x| x.as_str()).unwrap_or("");
                        let input = block.get("input").cloned().unwrap_or(Value::Null);
                        if !name.is_empty() {
                            content.push('\n');
                            content.push_str(&tool_call_markup(name, &input));
                        }
                    }
                    _ => content.push_str(block.get("text").and_then(|x| x.as_str()).unwrap_or("")),
                }
            }
        }
        Ok(split_think(content, thinking))
    }
}

fn curl_json(url: &str, key: &str, body: &Value, extra_headers: &[String]) -> Result<Value> {
    let req = std::env::temp_dir().join("ice-req.json");
    let mut f = std::fs::File::create(&req)?;
    f.write_all(body.to_string().as_bytes())?;
    drop(f);
    let mut cmd = Command::new("curl");
    cmd.args([
        "-sS",
        "-X",
        "POST",
        "-H",
        &format!("Authorization: Bearer {key}"),
        "-H",
        "Content-Type: application/json",
        "--data",
        &format!("@{}", req.display()),
        url,
    ]);
    for h in extra_headers {
        cmd.args(["-H", h]);
    }
    let out = cmd
        .output()
        .context("curl missing — install curl for live model calls")?;
    if !out.status.success() {
        bail!("curl failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    serde_json::from_slice(&out.stdout).context("llm json")
}

/// Render a native tool call as the `<function=…><parameter=…>` markup that
/// `Burst::parse` normalizes — one code path for every provider's format.
pub fn tool_call_markup(name: &str, args: &Value) -> String {
    let mut s = format!("<function={name}>");
    if let Some(obj) = args.as_object() {
        for (k, v) in obj {
            let val = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            s.push_str(&format!("<parameter={k}>{val}</parameter>"));
        }
    }
    s.push_str("</function>");
    s
}

fn split_think(content: String, mut thinking: String) -> Completion {
    let mut body = content;
    if let Some(start) = body.find("<think>") {
        if let Some(end) = body.find("</think>") {
            thinking = body[start + 7..end].trim().to_string();
            body = format!("{}{}", &body[..start], &body[end + 8..]);
        }
    }
    Completion {
        content: body.trim().to_string(),
        thinking,
    }
}

pub const SYSTEM_PROMPT: &str = r#"You are ICE, a fast terminal coding agent (like Claude Code, on a Rust harness).
You work step by step. Each turn you either CALL ONE OR MORE TOOLS, or, when the
task is fully finished, you write a short final answer with NO tool calls.

After every tool call you WILL be shown its result, then you continue. Never
guess a file's contents — read it. Never claim you did something you didn't do.

Emit tool calls as plain lines, one per line, exactly in this vocabulary:
  read <path>
  list <path>
  grep <pattern> <path>
  run <shell command>
  web_search <query>
  web_fetch <url>
  write <path>
  <<
  file contents
  >>
  replace <path>
  <<<<
  old text
  ====
  new text
  >>>>
  skill <name>
  mcp <server/tool> {json args}
  todo <text>   ·   todo_done <id>
  agent <type>: <delegated goal>

You may add one short sentence of plain text before your tool calls to say what
you are doing. When (and only when) the whole task is done, reply with a concise
final summary and DO NOT emit any tool line — that ends the task.

Rules:
- Take the smallest next step; don't dump a huge plan.
- To build something, actually WRITE the files with complete contents. For a
  website, write a self-contained index.html unless more is asked.
- Never sudo. Never escape the workspace.
- Delegate heavy or parallel work with `agent <type>: <goal>` (types: explore,
  plan, review = read-only; build, general). A subagent returns a short report.
- Do NOT write a `.ice/DONE` marker and do NOT use ASSERT blocks — you decide
  when the task is complete by simply stopping with a final answer.
- You MAY use XML tool tags (<function=…><parameter=…>) or the plain lines
  above; both are understood. Prefer the plain lines.

Example — "make me a website":
  write index.html
  <<
  <!doctype html><html><head><meta charset="utf-8"><title>Hello</title></head>
  <body><h1>Hello</h1><p>Built by ICE.</p></body></html>
  >>
Then, after seeing it was written, reply: "Done — created index.html with a simple landing page."
"#;

/// Lightweight system prompt for conversational replies — no schema, fast.
pub const CHAT_SYSTEM: &str = r#"You are ICE, a fast, friendly terminal coding agent.
Reply in plain, concise text — no schemas, no code fences unless code is asked for.
For greetings or small talk, answer in one or two short sentences.
If the user clearly wants you to build or change files, tell them briefly you're on it.
Never invent file changes here; this channel is just for talking."#;
