//! Hooks: user-defined shell commands run at lifecycle events, configured
//! in any settings layer the way Claude Code does:
//!
//! ```json
//! {"hooks": {"PreToolUse": [{"matcher": "Bash|Edit", "hooks": [{"type": "command", "command": "./check.sh"}]}]}}
//! ```
//!
//! The event JSON arrives on stdin. Exit code 0 = continue (stdout shown in
//! verbose mode); exit code 2 = block, and stderr is fed back to the model;
//! any other code is a non-blocking error shown to the user.

use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
struct Hook {
    event: String,
    matcher: String,
    command: String,
    timeout: Duration,
}

#[derive(Clone, Debug, Default)]
pub struct Hooks {
    list: Vec<Hook>,
}

#[derive(Debug, Default)]
pub struct Outcome {
    /// Exit code 2: block with this reason (for the model).
    pub block: Option<String>,
    /// Other non-zero exits / output for the user.
    pub notes: Vec<String>,
    /// Extra context to add (UserPromptSubmit / SessionStart stdout).
    pub context: Vec<String>,
}

impl Hooks {
    pub fn load(root: &Path) -> Hooks {
        let mut list = Vec::new();
        for p in [
            crate::settings::user_path(),
            crate::settings::project_path(root),
            crate::settings::local_path(root),
        ] {
            let Ok(t) = std::fs::read_to_string(&p) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<Value>(&t) else {
                continue;
            };
            let Some(events) = v["hooks"].as_object() else {
                continue;
            };
            for (event, groups) in events {
                for g in groups.as_array().cloned().unwrap_or_default() {
                    let matcher = g["matcher"].as_str().unwrap_or("").to_string();
                    for h in g["hooks"].as_array().cloned().unwrap_or_default() {
                        if h["type"].as_str().unwrap_or("command") != "command" {
                            continue;
                        }
                        if let Some(cmd) = h["command"].as_str() {
                            list.push(Hook {
                                event: event.clone(),
                                matcher: matcher.clone(),
                                command: cmd.to_string(),
                                timeout: Duration::from_secs(h["timeout"].as_u64().unwrap_or(60)),
                            });
                        }
                    }
                }
            }
        }
        Hooks { list }
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn count(&self) -> usize {
        self.list.len()
    }

    fn matches(h: &Hook, tool: Option<&str>) -> bool {
        match tool {
            None => true,
            Some(t) => {
                h.matcher.is_empty()
                    || h.matcher == "*"
                    || regex::Regex::new(&format!("^(?:{})$", h.matcher))
                        .map(|r| r.is_match(t))
                        .unwrap_or(h.matcher == t)
            }
        }
    }

    /// Run every hook for `event` (filtered by tool name when given).
    pub fn run(&self, root: &Path, event: &str, tool: Option<&str>, payload: Value) -> Outcome {
        let mut out = Outcome::default();
        for h in self
            .list
            .iter()
            .filter(|h| h.event == event && Self::matches(h, tool))
        {
            let mut body = payload.clone();
            body["hook_event_name"] = json!(event);
            body["cwd"] = json!(root.display().to_string());
            let shell = if cfg!(windows) {
                ("cmd", "/C")
            } else {
                ("sh", "-c")
            };
            let child = Command::new(shell.0)
                .arg(shell.1)
                .arg(&h.command)
                .current_dir(root)
                .env("ICE_PROJECT_DIR", root)
                .env("CLAUDE_PROJECT_DIR", root)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn();
            let Ok(mut child) = child else {
                out.notes
                    .push(format!("{event} hook failed to start: {}", h.command));
                continue;
            };
            if let Some(mut si) = child.stdin.take() {
                let _ = si.write_all(body.to_string().as_bytes());
            }
            let t0 = Instant::now();
            let status = loop {
                match child.try_wait() {
                    Ok(Some(s)) => break Some(s),
                    Ok(None) if t0.elapsed() > h.timeout => {
                        let _ = child.kill();
                        break None;
                    }
                    Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                    Err(_) => break None,
                }
            };
            let o = child.wait_with_output().ok();
            let stdout = o
                .as_ref()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();
            let stderr = o
                .as_ref()
                .map(|o| String::from_utf8_lossy(&o.stderr).trim().to_string())
                .unwrap_or_default();
            match status.and_then(|s| s.code()) {
                Some(0) => {
                    // JSON output may carry a decision.
                    if let Ok(v) = serde_json::from_str::<Value>(&stdout) {
                        let decision = v["decision"]
                            .as_str()
                            .or_else(|| v["hookSpecificOutput"]["permissionDecision"].as_str());
                        if matches!(decision, Some("block") | Some("deny")) {
                            out.block = Some(
                                v["reason"]
                                    .as_str()
                                    .or_else(|| {
                                        v["hookSpecificOutput"]["permissionDecisionReason"].as_str()
                                    })
                                    .unwrap_or("Blocked by hook")
                                    .to_string(),
                            );
                        }
                        if let Some(c) = v["hookSpecificOutput"]["additionalContext"].as_str() {
                            out.context.push(c.to_string());
                        }
                    } else if !stdout.is_empty()
                        && (event == "UserPromptSubmit" || event == "SessionStart")
                    {
                        out.context.push(stdout);
                    }
                }
                Some(2) => {
                    out.block = Some(if stderr.is_empty() {
                        format!("Blocked by {event} hook")
                    } else {
                        stderr
                    });
                }
                Some(c) => out.notes.push(format!(
                    "{event} hook `{}` exited with {c}: {stderr}",
                    h.command
                )),
                None => out
                    .notes
                    .push(format!("{event} hook `{}` timed out", h.command)),
            }
        }
        out
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn exit_two_blocks_with_stderr() {
        let root = std::env::temp_dir().join(format!("ice-hooks-{}", crate::conv::new_id("h")));
        std::fs::create_dir_all(root.join(".ice")).unwrap();
        std::fs::write(
            root.join(".ice/settings.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"grep -q rm && { echo 'no rm allowed' >&2; exit 2; } || exit 0"}]}]}}"#,
        )
        .unwrap();
        let h = Hooks::load(&root);
        assert_eq!(h.count(), 1);
        let o = h.run(
            &root,
            "PreToolUse",
            Some("Bash"),
            json!({"tool_input": {"command": "rm -rf x"}}),
        );
        assert_eq!(o.block.as_deref(), Some("no rm allowed"));
        let o = h.run(
            &root,
            "PreToolUse",
            Some("Bash"),
            json!({"tool_input": {"command": "ls"}}),
        );
        assert!(o.block.is_none());
        let o = h.run(&root, "PreToolUse", Some("Read"), json!({}));
        assert!(o.block.is_none());
    }
}
