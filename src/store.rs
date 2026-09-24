//! Session transcripts on disk, one JSONL file per session:
//!   ~/.ice/projects/<escaped project path>/<session-id>.jsonl
//! Each line is an event; replaying the file rebuilds the conversation
//! (from the last compaction boundary on). Powers --continue / --resume.

use crate::conv::{Message, Usage};
use anyhow::Result;
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub fn projects_dir() -> PathBuf {
    crate::settings::user_dir().join("projects")
}

/// `/home/me/my app` → `-home-me-my-app`
pub fn escape_path(p: &Path) -> String {
    p.display()
        .to_string()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

pub fn project_dir(root: &Path) -> PathBuf {
    projects_dir().join(escape_path(root))
}

pub fn now_iso() -> String {
    let d = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs() as i64;
    let (y, m, day) = crate::prompt::civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        d.subsec_millis()
    )
}

pub struct Transcript {
    pub id: String,
    pub path: PathBuf,
    root: PathBuf,
    disabled: bool,
}

impl Transcript {
    pub fn new(root: &Path, id: &str) -> Self {
        let dir = project_dir(root);
        Self {
            id: id.to_string(),
            path: dir.join(format!("{id}.jsonl")),
            root: root.to_path_buf(),
            disabled: std::env::var_os("ICE_NO_TRANSCRIPT").is_some(),
        }
    }

    /// A sub-agent's transcript, kept beside (not among) resumable sessions.
    pub fn sidechain(root: &Path, session: &str, agent: &str) -> Self {
        let mut t = Self::new(root, agent);
        t.path = project_dir(root)
            .join(format!("{session}.agents"))
            .join(format!("{agent}.jsonl"));
        t
    }

    fn append(&self, mut v: Value) {
        if self.disabled {
            return;
        }
        v["sessionId"] = json!(self.id);
        v["timestamp"] = json!(now_iso());
        v["cwd"] = json!(self.root.display().to_string());
        if let Some(d) = self.path.parent() {
            let _ = fs::create_dir_all(d);
        }
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(f, "{v}");
        }
    }

    pub fn message(&self, m: &Message) {
        let kind = match m.role {
            crate::conv::Role::User => "user",
            crate::conv::Role::Assistant => "assistant",
        };
        self.append(json!({"type": kind, "message": m}));
    }

    pub fn usage(&self, model: &str, u: &Usage) {
        self.append(json!({"type": "usage", "model": model, "usage": u}));
    }

    pub fn compact(&self, summary: &str) {
        self.append(json!({"type": "compact", "summary": summary}));
    }
}

#[derive(Clone, Debug)]
pub struct SessionInfo {
    pub id: String,
    pub modified: SystemTime,
    pub title: String,
    pub messages: usize,
}

/// Sessions for this project, newest first.
pub fn list(root: &Path) -> Vec<SessionInfo> {
    let dir = project_dir(root);
    let Ok(rd) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("jsonl") {
            continue;
        }
        let modified = e
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let Ok(txt) = fs::read_to_string(&p) else {
            continue;
        };
        let mut title = String::new();
        let mut first_prompt = String::new();
        let mut messages = 0;
        for line in txt.lines() {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            match v["type"].as_str() {
                Some("title") => title = v["title"].as_str().unwrap_or("").to_string(),
                Some("user") | Some("assistant") => {
                    messages += 1;
                    if first_prompt.is_empty() && v["type"] == "user" {
                        if let Ok(m) = serde_json::from_value::<Message>(v["message"].clone()) {
                            let t = m.text();
                            if !t.trim().is_empty() && !t.starts_with("<system-reminder>") {
                                first_prompt = t;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if messages == 0 {
            continue;
        }
        let title = if title.is_empty() {
            first_prompt
        } else {
            title
        };
        out.push(SessionInfo {
            id: p
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string(),
            modified,
            title: title
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(80)
                .collect(),
            messages,
        });
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out
}

/// Rebuild the conversation from a transcript.
pub fn load(path: &Path) -> Result<(Vec<Message>, Usage)> {
    let txt = fs::read_to_string(path)?;
    let mut msgs: Vec<Message> = Vec::new();
    let mut usage = Usage::default();
    for line in txt.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match v["type"].as_str() {
            Some("user") | Some("assistant") => {
                if let Ok(m) = serde_json::from_value::<Message>(v["message"].clone()) {
                    msgs.push(m);
                }
            }
            Some("compact") => {
                msgs.clear();
                msgs.push(compact_message(v["summary"].as_str().unwrap_or("")));
            }
            Some("usage") => {
                if let Ok(u) = serde_json::from_value::<Usage>(v["usage"].clone()) {
                    usage.add(&u);
                }
            }
            _ => {}
        }
    }
    repair(&mut msgs);
    Ok((msgs, usage))
}

pub fn compact_message(summary: &str) -> Message {
    Message::user_text(format!(
        "This session is being continued from a previous conversation that ran out of context. The conversation is summarized below:\n{summary}\n\nPlease continue the conversation from where we left it off without asking the user any further questions. Continue with the last task that you were asked to work on."
    ))
}

/// Make a (possibly interrupted) history valid for the API: every tool_use
/// needs a tool_result right after it, and roles must alternate.
pub fn repair(msgs: &mut Vec<Message>) {
    use crate::conv::{Block, Role};
    let mut i = 0;
    while i < msgs.len() {
        if msgs[i].role == Role::Assistant {
            let ids: Vec<String> = msgs[i]
                .tool_uses()
                .iter()
                .map(|(id, _, _)| id.to_string())
                .collect();
            if !ids.is_empty() {
                let answered: Vec<String> = msgs
                    .get(i + 1)
                    .filter(|m| m.role == Role::User)
                    .map(|m| {
                        m.content
                            .iter()
                            .filter_map(|b| match b {
                                Block::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
                                _ => None,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let missing: Vec<Block> = ids
                    .iter()
                    .filter(|id| !answered.contains(id))
                    .map(|id| Block::ToolResult {
                        tool_use_id: id.clone(),
                        content: "[Request interrupted by user for tool use]".into(),
                        is_error: true,
                    })
                    .collect();
                if !missing.is_empty() {
                    if answered.is_empty()
                        && msgs
                            .get(i + 1)
                            .map(|m| m.role != Role::User)
                            .unwrap_or(true)
                    {
                        msgs.insert(
                            i + 1,
                            Message {
                                role: Role::User,
                                content: missing,
                            },
                        );
                    } else if let Some(next) = msgs.get_mut(i + 1) {
                        if next.role == Role::User {
                            let mut c = missing;
                            c.extend(std::mem::take(&mut next.content));
                            next.content = c;
                        } else {
                            msgs.insert(
                                i + 1,
                                Message {
                                    role: Role::User,
                                    content: missing,
                                },
                            );
                        }
                    }
                }
            }
        }
        i += 1;
    }
    // Merge consecutive same-role messages (e.g. interrupted user turns).
    let mut merged: Vec<Message> = Vec::new();
    for m in msgs.drain(..) {
        match merged.last_mut() {
            Some(last) if last.role == m.role => last.content.extend(m.content),
            _ => merged.push(m),
        }
    }
    // Tool results must come first in a user message.
    for m in merged.iter_mut() {
        if m.role == Role::User {
            m.content
                .sort_by_key(|b| !matches!(b, Block::ToolResult { .. }));
        }
    }
    // A conversation must start with a user message.
    while merged
        .first()
        .map(|m| m.role == Role::Assistant)
        .unwrap_or(false)
    {
        merged.remove(0);
    }
    *msgs = merged;
}

/// Delete transcripts older than `days` (0 disables cleanup).
pub fn cleanup(days: u64) {
    if days == 0 {
        return;
    }
    let cutoff = SystemTime::now() - std::time::Duration::from_secs(days * 86_400);
    let Ok(projects) = fs::read_dir(projects_dir()) else {
        return;
    };
    for proj in projects.flatten() {
        let Ok(rd) = fs::read_dir(proj.path()) else {
            continue;
        };
        for e in rd.flatten() {
            if e.metadata()
                .and_then(|m| m.modified())
                .map(|t| t < cutoff)
                .unwrap_or(false)
            {
                let _ = fs::remove_file(e.path());
            }
        }
    }
}

/// Human "3 minutes ago".
pub fn ago(t: SystemTime) -> String {
    let s = SystemTime::now()
        .duration_since(t)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86_399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conv::{Block, Role};

    #[test]
    fn escape_and_iso() {
        assert_eq!(escape_path(Path::new("/home/me/my app")), "-home-me-my-app");
        let t = now_iso();
        assert_eq!(t.len(), 24);
        assert!(t.ends_with('Z'));
    }

    #[test]
    fn repair_closes_dangling_tool_calls() {
        let mut msgs = vec![
            Message::user_text("go"),
            Message {
                role: Role::Assistant,
                content: vec![Block::ToolUse {
                    id: "a".into(),
                    name: "Bash".into(),
                    input: json!({}),
                }],
            },
            Message::user_text("stop, do something else"),
        ];
        repair(&mut msgs);
        assert_eq!(msgs.len(), 3);
        assert!(
            matches!(&msgs[2].content[0], Block::ToolResult { tool_use_id, is_error: true, .. } if tool_use_id == "a")
        );
        assert!(matches!(&msgs[2].content[1], Block::Text { .. }));
    }

    #[test]
    fn transcript_roundtrip_with_compaction() {
        let dir = std::env::temp_dir().join(format!("ice-store-{}", crate::conv::new_id("s")));
        std::env::set_var("ICE_CONFIG_DIR", &dir);
        let root = dir.join("proj");
        let t = Transcript::new(&root, "sess1");
        t.message(&Message::user_text("hello there"));
        t.message(&Message::assistant_text("hi"));
        t.usage(
            "m",
            &Usage {
                input_tokens: 10,
                output_tokens: 5,
                ..Default::default()
            },
        );
        let (m, u) = load(&t.path).unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(u.output_tokens, 5);
        t.compact("SUMMARY");
        t.message(&Message::user_text("next"));
        let (m, _) = load(&t.path).unwrap();
        assert_eq!(m.len(), 1, "compact summary and next user msg merge");
        assert!(m[0].text().contains("SUMMARY"));
        let l = list(&root);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].title, "hello there");
        std::env::remove_var("ICE_CONFIG_DIR");
    }
}
