//! Built-in LLM providers + installed-CLI adapters + model discovery.

use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Provider {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    pub key_envs: &'static [&'static str],
    pub default_model: &'static str,
    pub openai_compat: bool,
}

pub const PROVIDERS: &[Provider] = &[
    Provider {
        id: "xai",
        name: "xAI",
        base_url: "https://api.x.ai/v1",
        key_envs: &["XAI_API_KEY", "ICE_API_KEY"],
        default_model: "grok-3",
        openai_compat: true,
    },
    Provider {
        id: "openai",
        name: "OpenAI",
        base_url: "https://api.openai.com/v1",
        key_envs: &["OPENAI_API_KEY", "ICE_API_KEY"],
        default_model: "gpt-4o",
        openai_compat: true,
    },
    Provider {
        id: "groq",
        name: "Groq",
        base_url: "https://api.groq.com/openai/v1",
        key_envs: &["GROQ_API_KEY", "ICE_API_KEY"],
        default_model: "qwen/qwen3.8-27b",
        openai_compat: true,
    },
    Provider {
        id: "openrouter",
        name: "OpenRouter",
        base_url: "https://openrouter.ai/api/v1",
        key_envs: &["OPENROUTER_API_KEY", "ICE_API_KEY"],
        default_model: "openrouter/auto",
        openai_compat: true,
    },
    Provider {
        id: "together",
        name: "Together",
        base_url: "https://api.together.xyz/v1",
        key_envs: &["TOGETHER_API_KEY", "ICE_API_KEY"],
        default_model: "meta-llama/Llama-3.3-70B-Instruct-Turbo",
        openai_compat: true,
    },
    Provider {
        id: "fireworks",
        name: "Fireworks",
        base_url: "https://api.fireworks.ai/inference/v1",
        key_envs: &["FIREWORKS_API_KEY", "ICE_API_KEY"],
        default_model: "accounts/fireworks/models/llama-v3p3-70b-instruct",
        openai_compat: true,
    },
    Provider {
        id: "deepseek",
        name: "DeepSeek",
        base_url: "https://api.deepseek.com/v1",
        key_envs: &["DEEPSEEK_API_KEY", "ICE_API_KEY"],
        default_model: "deepseek-chat",
        openai_compat: true,
    },
    Provider {
        id: "mistral",
        name: "Mistral",
        base_url: "https://api.mistral.ai/v1",
        key_envs: &["MISTRAL_API_KEY", "ICE_API_KEY"],
        default_model: "mistral-large-latest",
        openai_compat: true,
    },
    Provider {
        id: "anthropic",
        name: "Anthropic",
        base_url: "https://api.anthropic.com/v1",
        key_envs: &["ANTHROPIC_API_KEY", "ICE_API_KEY"],
        default_model: "claude-sonnet-4-5",
        openai_compat: false,
    },
    Provider {
        id: "gemini",
        name: "Google Gemini",
        base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        key_envs: &["GEMINI_API_KEY", "GOOGLE_API_KEY", "ICE_API_KEY"],
        default_model: "gemini-2.5-flash",
        openai_compat: true,
    },
    Provider {
        id: "ollama",
        name: "Ollama (local)",
        base_url: "http://127.0.0.1:11434/v1",
        key_envs: &["OLLAMA_API_KEY", "ICE_API_KEY"],
        default_model: "llama3.1",
        openai_compat: true,
    },
    Provider {
        id: "custom",
        name: "Custom OpenAI-compatible",
        base_url: "http://127.0.0.1:8000/v1",
        key_envs: &["ICE_API_KEY", "OPENAI_API_KEY"],
        default_model: "local-model",
        openai_compat: true,
    },
];

/// A CLI coding agent ICE can drive as its backend, reusing the user's own
/// login/subscription in that tool instead of an API key.
#[derive(Clone, Copy, Debug)]
pub struct CliAgent {
    pub id: &'static str,
    pub name: &'static str,
    pub bin: &'static str,
    /// argv passed to the binary; "{PROMPT}" is replaced with the message.
    pub args: &'static [&'static str],
    /// One-line hint on how to authenticate the CLI.
    pub login_hint: &'static str,
    /// True for the "Sign in with ChatGPT" path.
    pub chatgpt: bool,
}

pub const CLI_AGENTS: &[CliAgent] = &[
    CliAgent {
        id: "codex",
        name: "ChatGPT · Codex CLI",
        bin: "codex",
        args: &["exec", "{PROMPT}"],
        login_hint: "Run `codex login` and choose “Sign in with ChatGPT”.",
        chatgpt: true,
    },
    CliAgent {
        id: "claude",
        name: "Claude · Claude Code CLI",
        bin: "claude",
        args: &["-p", "{PROMPT}"],
        login_hint: "Run `claude` once and sign in (Anthropic account or key).",
        chatgpt: false,
    },
    CliAgent {
        id: "opencode",
        name: "opencode",
        bin: "opencode",
        args: &["run", "{PROMPT}"],
        login_hint: "Run `opencode auth login` to connect a provider.",
        chatgpt: false,
    },
    CliAgent {
        id: "gemini",
        name: "Gemini CLI",
        bin: "gemini",
        args: &["-p", "{PROMPT}"],
        login_hint: "Run `gemini` once to sign in with your Google account.",
        chatgpt: false,
    },
    CliAgent {
        id: "qwen",
        name: "Qwen Code CLI",
        bin: "qwen",
        args: &["-p", "{PROMPT}"],
        login_hint: "Run `qwen` once to authenticate.",
        chatgpt: false,
    },
];

pub fn find_cli_agent(id: &str) -> Option<&'static CliAgent> {
    let id = id.strip_prefix("cli:").unwrap_or(id);
    CLI_AGENTS.iter().find(|c| c.id.eq_ignore_ascii_case(id))
}

/// The CLI agent selected as the active backend, if the config provider is
/// `cli:<id>`.
pub fn selected_cli_agent() -> Option<&'static CliAgent> {
    let cfg = IceConfig::load();
    if cfg.provider.starts_with("cli:") {
        find_cli_agent(&cfg.provider)
    } else {
        None
    }
}

pub fn cli_agent_present(c: &CliAgent) -> bool {
    which(c.bin)
}

#[derive(Clone, Debug)]
pub struct InstalledCli {
    pub id: &'static str,
    pub bin: &'static str,
    pub present: bool,
}

pub fn detect_clis() -> Vec<InstalledCli> {
    const BINS: &[(&str, &str)] = &[
        ("ice", "ice"),
        ("grok", "grok"),
        ("claude", "claude"),
        ("codex", "codex"),
        ("aider", "aider"),
        ("goose", "goose"),
        ("gemini", "gemini"),
        ("qwen", "qwen"),
        ("opencode", "opencode"),
        ("cursor", "cursor"),
        ("ollama", "ollama"),
    ];
    BINS.iter()
        .map(|(id, bin)| InstalledCli {
            id,
            bin,
            present: which(bin),
        })
        .collect()
}

fn which(bin: &str) -> bool {
    Command::new("sh")
        .args(["-lc", &format!("command -v {bin} >/dev/null 2>&1")])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn find(id: &str) -> Option<&'static Provider> {
    PROVIDERS
        .iter()
        .find(|p| p.id.eq_ignore_ascii_case(id) || p.name.eq_ignore_ascii_case(id))
}

pub fn key_for(p: &Provider) -> Option<String> {
    for env in p.key_envs {
        if let Ok(v) = std::env::var(env) {
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

pub fn detect_ready() -> Vec<(&'static Provider, String)> {
    PROVIDERS
        .iter()
        .filter_map(|p| key_for(p).map(|k| (p, k)))
        .collect()
}

pub fn list_models(p: &Provider) -> Result<Vec<String>> {
    if p.id == "ollama" {
        return list_ollama();
    }
    let key = key_for(p).unwrap_or_default();
    if key.is_empty() && p.id != "ollama" {
        bail!("{}: no API key in {:?}", p.name, p.key_envs);
    }
    let url = format!("{}/models", p.base_url.trim_end_matches('/'));
    let mut args = vec![
        "-sS",
        "--max-time",
        "20",
        "-H",
        "Content-Type: application/json",
    ];
    let auth = format!("Authorization: Bearer {key}");
    if !key.is_empty() {
        args.push("-H");
        args.push(&auth);
    }
    args.push(&url);
    let out = Command::new("curl")
        .args(&args)
        .output()
        .context("curl required for model discovery")?;
    if !out.status.success() {
        bail!(
            "models fetch failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let v: Value = serde_json::from_slice(&out.stdout).context("models json")?;
    let mut ids = Vec::new();
    if let Some(arr) = v.get("data").and_then(|d| d.as_array()) {
        for m in arr {
            if let Some(id) = m.get("id").and_then(|x| x.as_str()) {
                ids.push(id.to_string());
            }
        }
    } else if let Some(arr) = v.get("models").and_then(|d| d.as_array()) {
        for m in arr {
            if let Some(id) = m
                .get("name")
                .or_else(|| m.get("id"))
                .and_then(|x| x.as_str())
            {
                ids.push(id.to_string());
            }
        }
    }
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        ids.push(p.default_model.to_string());
    }
    Ok(ids)
}

fn list_ollama() -> Result<Vec<String>> {
    let out = Command::new("curl")
        .args(["-sS", "--max-time", "8", "http://127.0.0.1:11434/api/tags"])
        .output()?;
    if !out.status.success() {
        return Ok(vec!["llama3.1".into()]);
    }
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    let mut ids = Vec::new();
    if let Some(arr) = v.get("models").and_then(|d| d.as_array()) {
        for m in arr {
            if let Some(n) = m.get("name").and_then(|x| x.as_str()) {
                ids.push(n.to_string());
            }
        }
    }
    if ids.is_empty() {
        ids.push("llama3.1".into());
    }
    Ok(ids)
}

#[derive(Clone, Debug)]
pub struct IceConfig {
    pub provider: String,
    pub model: String,
    pub base_url: Option<String>,
    pub onboarded: bool,
}

impl Default for IceConfig {
    fn default() -> Self {
        Self {
            provider: "xai".into(),
            model: "grok-3".into(),
            base_url: None,
            onboarded: false,
        }
    }
}

impl IceConfig {
    pub fn path() -> std::path::PathBuf {
        if let Ok(h) = std::env::var("HOME") {
            std::path::PathBuf::from(h).join(".ice/config.json")
        } else {
            std::path::PathBuf::from(".ice/config.json")
        }
    }

    pub fn load() -> Self {
        let p = Self::path();
        let Ok(txt) = std::fs::read_to_string(&p) else {
            return Self::from_env();
        };
        let v: Value = serde_json::from_str(&txt).unwrap_or(Value::Null);
        let mut c = Self::from_env();
        if let Some(s) = v.get("provider").and_then(|x| x.as_str()) {
            c.provider = s.to_string();
        }
        if let Some(s) = v.get("model").and_then(|x| x.as_str()) {
            c.model = s.to_string();
        }
        if let Some(s) = v.get("base_url").and_then(|x| x.as_str()) {
            c.base_url = Some(s.to_string());
        }
        c.onboarded = v
            .get("onboarded")
            .and_then(|x| x.as_bool())
            .unwrap_or(false);
        c
    }

    pub fn from_env() -> Self {
        let mut c = Self::default();
        if let Ok(m) = std::env::var("ICE_MODEL") {
            c.model = m;
        }
        if let Ok(b) = std::env::var("ICE_BASE_URL") {
            c.base_url = Some(b);
        }
        if let Ok(p) = std::env::var("ICE_PROVIDER") {
            c.provider = p;
        } else if std::env::var("GROQ_API_KEY").is_ok() {
            c.provider = "groq".into();
        } else if std::env::var("OPENAI_API_KEY").is_ok() {
            c.provider = "openai".into();
        } else if std::env::var("ANTHROPIC_API_KEY").is_ok() {
            c.provider = "anthropic".into();
        }
        c
    }

    pub fn save(&self) -> Result<()> {
        let p = Self::path();
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let v = serde_json::json!({
            "provider": self.provider,
            "model": self.model,
            "base_url": self.base_url,
            "onboarded": self.onboarded,
        });
        std::fs::write(p, serde_json::to_string_pretty(&v)?)?;
        Ok(())
    }

    pub fn apply_env(&self) {
        std::env::set_var("ICE_PROVIDER", &self.provider);
        std::env::set_var("ICE_MODEL", &self.model);
        if let Some(b) = &self.base_url {
            std::env::set_var("ICE_BASE_URL", b);
        } else if let Some(p) = find(&self.provider) {
            std::env::set_var("ICE_BASE_URL", p.base_url);
        }
    }
}
