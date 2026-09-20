//! Workspace bootstrap: write a full `.ice/` tree on any path.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub const SCHEMA: &str = "ice.workspace/v1";

pub fn workspace_dir(root: &Path) -> PathBuf {
    root.join(".ice")
}

/// Create (or refresh missing files in) `.ice` under `root`.
/// Never overwrites user-edited settings/config/mcp/todos/goal.
pub fn ensure(root: &Path) -> Result<PathBuf> {
    let ice = workspace_dir(root);
    fs::create_dir_all(&ice).with_context(|| format!("mkdir {}", ice.display()))?;
    for sub in [
        "skills/ice-burst",
        "agents",
        "mcp",
        "logs",
        "sessions",
        "cache",
        "exports",
    ] {
        fs::create_dir_all(ice.join(sub))?;
    }

    write_if_missing(&ice.join("settings.json"), SETTINGS)?;
    write_if_missing(&ice.join("config.json"), CONFIG)?;
    write_if_missing(&ice.join("providers.json"), PROVIDERS)?;
    write_if_missing(&ice.join("mcp.json"), MCP)?;
    write_if_missing(&ice.join("todos.json"), "[]\n")?;
    write_if_missing(&ice.join("gitignore"), GITIGNORE)?;
    write_if_missing(&ice.join("README.md"), ICE_README)?;
    write_if_missing(&ice.join("goal.md"), GOAL)?;
    write_if_missing(&ice.join("progress.md"), "# progress\n\n")?;
    write_if_missing(&ice.join("skills/ice-burst/SKILL.md"), SKILL)?;
    write_if_missing(&ice.join("mcp/echo.py"), MCP_ECHO)?;
    write_if_missing(&ice.join(".schema"), &format!("{SCHEMA}\n"))?;
    Ok(ice)
}

fn write_if_missing(path: &Path, body: &str) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(d) = path.parent() {
        fs::create_dir_all(d)?;
    }
    fs::write(path, body).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

pub fn describe(root: &Path) -> String {
    let ice = workspace_dir(root);
    format!(
        "ICE workspace  {SCHEMA}\n\
         root      {}\n\
         .ice      {}\n\
         settings  .ice/settings.json\n\
         config    .ice/config.json\n\
         providers .ice/providers.json\n\
         mcp       .ice/mcp.json\n\
         skills    .ice/skills/\n\
         todos     .ice/todos.json\n\
         goal      .ice/goal.md\n\
         agents    .ice/agents/\n\
         sessions  .ice/sessions/\n\
         logs      .ice/logs/",
        root.display(),
        ice.display()
    )
}

const SETTINGS: &str = r#"{
  "schema": "ice.workspace/v1",
  "harness": {
    "name": "ICE",
    "loop": ["intent", "compile", "execute", "verify", "delta"],
    "max_turns": 4,
    "max_loops": 8,
    "yolo": false,
    "demo": false,
    "show_thinking": true,
    "subagent_depth": 2
  },
  "ui": {
    "theme": "ice",
    "themes": ["ice", "groknight", "frost", "ember", "mono"],
    "input_widget": true,
    "todo_panel": true,
    "context_bar": true,
    "onboard_on_first_run": true
  },
  "context": {
    "window_tokens": 131072,
    "estimator": "chars/4"
  },
  "sandbox": {
    "deny": ["sudo", "rm -rf /", "mkfs", "shutdown"],
    "timeout_sec": 30
  },
  "paths": {
    "goal": ".ice/goal.md",
    "progress": ".ice/progress.md",
    "done": ".ice/DONE",
    "todos": ".ice/todos.json",
    "agents": ".ice/agents",
    "skills": ".ice/skills",
    "mcp": ".ice/mcp.json",
    "logs": ".ice/logs",
    "sessions": ".ice/sessions"
  }
}
"#;

const CONFIG: &str = r#"{
  "provider": "groq",
  "model": "qwen/qwen3.8-27b",
  "base_url": "https://api.groq.com/openai/v1",
  "onboarded": false
}
"#;

const PROVIDERS: &str = r#"{
  "ready_env": [
    "XAI_API_KEY",
    "OPENAI_API_KEY",
    "GROQ_API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENROUTER_API_KEY",
    "TOGETHER_API_KEY",
    "FIREWORKS_API_KEY",
    "DEEPSEEK_API_KEY",
    "MISTRAL_API_KEY",
    "GEMINI_API_KEY",
    "ICE_API_KEY"
  ],
  "defaults": {
    "xai":        { "base": "https://api.x.ai/v1", "model": "grok-3" },
    "openai":     { "base": "https://api.openai.com/v1", "model": "gpt-4o" },
    "groq":       { "base": "https://api.groq.com/openai/v1", "model": "qwen/qwen3.8-27b" },
    "openrouter": { "base": "https://openrouter.ai/api/v1", "model": "openrouter/auto" },
    "together":   { "base": "https://api.together.xyz/v1", "model": "meta-llama/Llama-3.3-70B-Instruct-Turbo" },
    "fireworks":  { "base": "https://api.fireworks.ai/inference/v1", "model": "accounts/fireworks/models/llama-v3p3-70b-instruct" },
    "deepseek":   { "base": "https://api.deepseek.com/v1", "model": "deepseek-chat" },
    "mistral":    { "base": "https://api.mistral.ai/v1", "model": "mistral-large-latest" },
    "anthropic":  { "base": "https://api.anthropic.com/v1", "model": "claude-sonnet-4-5" },
    "gemini":     { "base": "https://generativelanguage.googleapis.com/v1beta/openai", "model": "gemini-2.5-flash" },
    "ollama":     { "base": "http://127.0.0.1:11434/v1", "model": "llama3.1" },
    "custom":     { "base": "http://127.0.0.1:8000/v1", "model": "local-model" }
  }
}
"#;

const MCP: &str = r#"{
  "mcpServers": {
    "echo": {
      "command": "python3",
      "args": [".ice/mcp/echo.py"]
    }
  }
}
"#;

const GITIGNORE: &str = "logs/\ncache/\nsessions/\nexports/\n";

const ICE_README: &str = r#"# .ice workspace

Created by `ice init` / first CLI launch.

| file | role |
| --- | --- |
| settings.json | harness + UI + sandbox knobs |
| config.json | active provider / model (no secrets) |
| providers.json | catalog of endpoints |
| mcp.json | MCP stdio servers |
| todos.json | live todo list |
| goal.md | durable /goal |
| progress.md | /loop log |
| skills/ | SKILL.md packs |
| agents/ | subagent receipts |
| logs/ | trajectories |
| sessions/ | TUI session ids |
"#;

const GOAL: &str = r#"# goal

(none yet — `/goal <text> until file_exists .ice/DONE`)

## DONE WHEN

- file_exists .ice/DONE
"#;

const SKILL: &str = r#"# ice-burst

Prefer one compiled BURST over many tool calls.
Always include an ASSERT that can fail.
Use `todo` for multi-step work and `todo_done` when a step lands.
Use `mcp echo/ping` to check MCP connectivity.
"#;

const MCP_ECHO: &str = r#"#!/usr/bin/env python3
import json, sys

def send(o):
    sys.stdout.write(json.dumps(o) + "\n")
    sys.stdout.flush()

def handle(req):
    mid, method, params = req.get("id"), req.get("method"), req.get("params") or {}
    if method == "initialize":
        return {"jsonrpc":"2.0","id":mid,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"ice-echo","version":"0.1"}}}
    if method == "tools/list":
        return {"jsonrpc":"2.0","id":mid,"result":{"tools":[{"name":"ping","description":"echo a message","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}}]}}
    if method == "tools/call":
        text = (params.get("arguments") or {}).get("text", "pong")
        return {"jsonrpc":"2.0","id":mid,"result":{"content":[{"type":"text","text":"echo:"+text}],"isError":False}}
    return None

if __name__ == "__main__":
    for line in sys.stdin:
        line = line.strip()
        if not line: continue
        try: req = json.loads(line)
        except Exception: continue
        resp = handle(req)
        if resp is not None: send(resp)
"#;
