//! Built-in LLM providers + installed-CLI adapters + model discovery.

use anyhow::{bail, Context, Result};
use serde_json::Value;

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
        default_model: "llama-3.3-70b-versatile",
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
}

pub const CLI_AGENTS: &[CliAgent] = &[
    CliAgent {
        id: "codex",
        name: "ChatGPT · Codex CLI",
        bin: "codex",
        args: &["exec", "{PROMPT}"],
        login_hint: "Run `codex login` and choose “Sign in with ChatGPT”.",
    },
    CliAgent {
        id: "claude",
        name: "Claude · Claude Code CLI",
        bin: "claude",
        args: &["-p", "{PROMPT}"],
        login_hint: "Run `claude` once and sign in (Anthropic account or key).",
    },
    CliAgent {
        id: "opencode",
        name: "opencode",
        bin: "opencode",
        args: &["run", "{PROMPT}"],
        login_hint: "Run `opencode auth login` to connect a provider.",
    },
    CliAgent {
        id: "gemini",
        name: "Gemini CLI",
        bin: "gemini",
        args: &["-p", "{PROMPT}"],
        login_hint: "Run `gemini` once to sign in with your Google account.",
    },
    CliAgent {
        id: "qwen",
        name: "Qwen Code CLI",
        bin: "qwen",
        args: &["-p", "{PROMPT}"],
        login_hint: "Run `qwen` once to authenticate.",
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

fn which(bin: &str) -> bool {
    crate::tools::which(bin)
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

pub fn list_models(p: &Provider) -> Result<Vec<String>> {
    if p.id == "ollama" {
        return list_ollama();
    }
    let key = key_for(p).unwrap_or_default();
    if key.is_empty() {
        bail!(
            "{}: no API key (set {} or run /login)",
            p.name,
            p.key_envs[0]
        );
    }
    let base = std::env::var("ICE_BASE_URL")
        .ok()
        .filter(|b| !b.is_empty() && IceConfig::load().provider == p.id)
        .unwrap_or_else(|| p.base_url.to_string());
    let url = format!("{}/models", base.trim_end_matches('/'));
    let auth = format!("Bearer {key}");
    let mut headers: Vec<(&str, &str)> = vec![];
    if p.id == "anthropic" {
        headers.push(("x-api-key", key.as_str()));
        headers.push(("anthropic-version", "2023-06-01"));
    } else {
        headers.push(("authorization", auth.as_str()));
    }
    let v: Value = crate::http::get_json(&url, &headers, std::time::Duration::from_secs(20))
        .context("model discovery failed")?;
    let mut ids = Vec::new();
    for arr in [v.get("data"), v.get("models")]
        .into_iter()
        .flatten()
        .filter_map(|a| a.as_array())
    {
        for m in arr {
            if let Some(id) = m
                .get("id")
                .or_else(|| m.get("name"))
                .and_then(|x| x.as_str())
            {
                ids.push(id.trim_start_matches("models/").to_string());
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
    let host = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://127.0.0.1:11434".into());
    let host = if host.starts_with("http") {
        host
    } else {
        format!("http://{host}")
    };
    let v = crate::http::get_json(
        &format!("{}/api/tags", host.trim_end_matches('/')),
        &[],
        std::time::Duration::from_secs(5),
    )
    .context("Ollama is not reachable — is `ollama serve` running?")?;
    let mut ids: Vec<String> = v
        .get("models")
        .and_then(|d| d.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|m| m.get("name").and_then(|x| x.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if ids.is_empty() {
        ids.push("llama3.1".into());
    }
    Ok(ids)
}

/// ~/.ice/config.json: the active provider/model plus per-project state
/// (trust). Unknown keys are preserved on save.
#[derive(Clone, Debug)]
pub struct IceConfig {
    pub provider: String,
    pub model: String,
    pub base_url: Option<String>,
    pub onboarded: bool,
}

/// Provider chosen from whichever key is present, in this order.
const ENV_ORDER: &[(&str, &str)] = &[
    ("ANTHROPIC_API_KEY", "anthropic"),
    ("OPENAI_API_KEY", "openai"),
    ("GEMINI_API_KEY", "gemini"),
    ("GOOGLE_API_KEY", "gemini"),
    ("XAI_API_KEY", "xai"),
    ("GROQ_API_KEY", "groq"),
    ("OPENROUTER_API_KEY", "openrouter"),
    ("DEEPSEEK_API_KEY", "deepseek"),
    ("MISTRAL_API_KEY", "mistral"),
    ("TOGETHER_API_KEY", "together"),
    ("FIREWORKS_API_KEY", "fireworks"),
];

impl IceConfig {
    pub fn path() -> std::path::PathBuf {
        crate::settings::user_dir().join("config.json")
    }

    fn raw() -> Value {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null)
    }

    pub fn load() -> Self {
        let v = Self::raw();
        let mut c = Self::from_env();
        if std::env::var("ICE_PROVIDER").is_err() {
            if let Some(s) = v.get("provider").and_then(|x| x.as_str()) {
                // A saved provider only applies if its key is available
                // (or it needs none); otherwise fall back to detection.
                let usable = s.starts_with("cli:")
                    || find(s)
                        .map(|p| key_for(p).is_some() || p.id == "ollama" || p.id == "custom")
                        .unwrap_or(false);
                if usable || c.provider.is_empty() {
                    c.provider = s.to_string();
                    c.model = v
                        .get("model")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    c.base_url = v.get("base_url").and_then(|x| x.as_str()).map(String::from);
                }
            }
        }
        if std::env::var("ICE_MODEL").is_err() && c.model.is_empty() {
            if let Some(p) = find(&c.provider) {
                c.model = p.default_model.to_string();
            }
        }
        c.onboarded = v
            .get("onboarded")
            .and_then(|x| x.as_bool())
            .unwrap_or(false);
        c
    }

    pub fn from_env() -> Self {
        let mut c = IceConfig {
            provider: String::new(),
            model: String::new(),
            base_url: None,
            onboarded: false,
        };
        if let Ok(p) = std::env::var("ICE_PROVIDER") {
            c.provider = p;
        } else if let Some((_, id)) = ENV_ORDER
            .iter()
            .find(|(k, _)| std::env::var(k).map(|v| !v.is_empty()).unwrap_or(false))
        {
            c.provider = id.to_string();
        }
        if let Ok(m) = std::env::var("ICE_MODEL") {
            c.model = m;
        }
        if let Ok(b) = std::env::var("ICE_BASE_URL") {
            c.base_url = Some(b);
        }
        c
    }

    pub fn save(&self) -> Result<()> {
        let p = Self::path();
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut v = Self::raw();
        if !v.is_object() {
            v = serde_json::json!({});
        }
        v["provider"] = serde_json::json!(self.provider);
        v["model"] = serde_json::json!(self.model);
        v["base_url"] = serde_json::json!(self.base_url);
        v["onboarded"] = serde_json::json!(self.onboarded);
        std::fs::write(p, serde_json::to_string_pretty(&v)? + "\n")?;
        Ok(())
    }

    pub fn is_trusted(root: &std::path::Path) -> bool {
        let v = Self::raw();
        let key = root.display().to_string();
        v["projects"][&key]["trusted"].as_bool().unwrap_or(false)
            || root.ancestors().skip(1).any(|a| {
                v["projects"][&a.display().to_string()]["trusted"]
                    .as_bool()
                    .unwrap_or(false)
            })
    }

    pub fn set_trusted(root: &std::path::Path) -> Result<()> {
        let p = Self::path();
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut v = Self::raw();
        if !v.is_object() {
            v = serde_json::json!({});
        }
        if !v["projects"].is_object() {
            v["projects"] = serde_json::json!({});
        }
        v["projects"][root.display().to_string()] = serde_json::json!({"trusted": true});
        std::fs::write(p, serde_json::to_string_pretty(&v)? + "\n")?;
        Ok(())
    }
}
