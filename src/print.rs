//! Print mode (`ice -p`): run one prompt non-interactively and write the
//! result as text, JSON, or a stream of JSON events — for scripts and CI.

use crate::engine::{Engine, Event, Options};
use serde_json::json;
use std::io::{IsTerminal, Read, Write};
use std::sync::Mutex;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Text,
    Json,
    StreamJson,
}

/// Combine an explicit prompt with piped stdin (`cat log | ice -p "why?"`).
/// When a prompt is given, stdin is only waited on briefly, so scripts that
/// leave stdin open (CI runners, IDE tasks) don't hang.
pub fn read_prompt(prompt: Option<String>) -> String {
    let mut piped = String::new();
    if !std::io::stdin().is_terminal() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = String::new();
            let _ = std::io::stdin().read_to_string(&mut buf);
            let _ = tx.send(buf);
        });
        piped = if prompt.is_some() {
            rx.recv_timeout(std::time::Duration::from_millis(1500))
                .unwrap_or_default()
        } else {
            rx.recv().unwrap_or_default()
        };
    }
    match (prompt, piped.trim().is_empty()) {
        (Some(p), true) => p,
        (Some(p), false) => format!("{}\n\n{p}", piped.trim_end()),
        (None, false) => piped,
        (None, true) => String::new(),
    }
}

pub fn run(
    opts: Options,
    prompt: String,
    format: Format,
    verbose: bool,
    resume: Option<String>,
) -> i32 {
    if prompt.trim().is_empty() {
        eprintln!("Error: Input must be provided either through stdin or as a prompt argument when using --print");
        return 1;
    }
    let mut eng = Engine::new(opts);
    if let Some(id) = resume {
        if let Err(e) = eng.resume(&id) {
            eprintln!("Error: {e}");
            return 1;
        }
    }
    let out = Mutex::new(std::io::stdout());
    let line = |v: serde_json::Value| {
        let mut o = out.lock().unwrap();
        let _ = writeln!(o, "{v}");
        let _ = o.flush();
    };
    let session = eng.session_id.clone();
    if format == Format::StreamJson {
        let tools: Vec<String> = crate::tools::builtin_specs(&[])
            .into_iter()
            .map(|t| t.name)
            .chain(eng.mcp.specs().into_iter().map(|t| t.name))
            .collect();
        line(json!({
            "type": "system", "subtype": "init", "session_id": session, "cwd": eng.root.display().to_string(),
            "model": eng.model_name(), "tools": tools, "permissionMode": eng.mode.id(),
            "mcp_servers": eng.mcp.status.iter().map(|(n, s)| json!({"name": n, "status": if s.is_ok() { "connected" } else { "failed" }})).collect::<Vec<_>>(),
        }));
    }
    let color = std::io::stderr().is_terminal() && !crate::theme::no_color();
    let root = eng.root.clone();
    let emit = |e: Event| match (&e, format) {
        (Event::Text(t), Format::StreamJson) => line(
            json!({"type": "assistant", "session_id": session, "message": {"role": "assistant", "content": [{"type": "text", "text": t}]}}),
        ),
        (Event::ToolStart { id, name, input }, Format::StreamJson) => line(
            json!({"type": "assistant", "session_id": session, "message": {"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": name, "input": input}]}}),
        ),
        (Event::ToolDone { id, output, .. }, Format::StreamJson) => line(
            json!({"type": "user", "session_id": session, "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": output.content, "is_error": output.is_error}]}}),
        ),
        (Event::ToolStart { name, input, .. }, _) if verbose => {
            let t = crate::render::tool_call_title(name, input, &root);
            if color {
                eprintln!("\x1b[2m⏺ {t}\x1b[0m");
            } else {
                eprintln!("⏺ {t}");
            }
        }
        (
            Event::Retry {
                attempt,
                error,
                wait,
            },
            _,
        ) if verbose => eprintln!(
            "retrying in {}s (attempt {attempt}): {error}",
            wait.as_secs()
        ),
        (Event::Notice(n), _) if verbose => eprintln!("{n}"),
        _ => {}
    };
    let r = eng.submit(&prompt, &emit);
    let stats = eng.stats();
    let subtype = if !r.is_error {
        "success"
    } else if r.text.starts_with("Reached max turns") {
        "error_max_turns"
    } else {
        "error_during_execution"
    };
    match format {
        Format::Text => {
            if r.is_error {
                eprintln!("Error: {}", r.text);
            } else {
                println!("{}", r.text);
            }
        }
        Format::Json | Format::StreamJson => line(json!({
            "type": "result",
            "subtype": subtype,
            "is_error": r.is_error,
            "duration_ms": r.duration_ms,
            "duration_api_ms": stats.api_ms,
            "num_turns": r.num_turns,
            "result": r.text,
            "session_id": stats.session_id,
            "total_cost_usd": stats.cost,
            "usage": stats.usage,
        })),
    }
    if r.is_error {
        1
    } else {
        0
    }
}
