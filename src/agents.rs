//! Sub-agent definitions for the Task tool: the built-in types plus custom
//! agents defined as markdown files with frontmatter, in
//! `.ice/agents/`, `.claude/agents/` (compatible) and `~/.ice/agents/`:
//!
//! ```markdown
//! ---
//! name: reviewer
//! description: Reviews diffs for bugs. Use after writing code.
//! tools: Read, Grep, Glob
//! model: inherit
//! ---
//! You are a meticulous code reviewer…
//! ```

use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct AgentDef {
    pub name: String,
    pub description: String,
    /// None = all tools (except Task).
    pub tools: Option<Vec<String>>,
    pub model: Option<String>,
    pub prompt: String,
    pub source: String,
}

const READ_TOOLS: &[&str] = &[
    "Read",
    "Glob",
    "Grep",
    "LS",
    "WebFetch",
    "WebSearch",
    "BashOutput",
    "Skill",
];

pub fn builtins() -> Vec<AgentDef> {
    vec![
        AgentDef {
            name: "general-purpose".into(),
            description: "General-purpose agent for researching complex questions, searching for code, and executing multi-step tasks.".into(),
            tools: None,
            model: None,
            prompt: "You are a general-purpose agent. Complete the task fully — don't gold-plate, but don't leave it half-done.".into(),
            source: "built-in".into(),
        },
        AgentDef {
            name: "Explore".into(),
            description: "Fast read-only agent specialized for exploring codebases: find files by pattern, search code for keywords, answer questions about the codebase. Say how thorough to be: quick, medium or very thorough.".into(),
            tools: Some(READ_TOOLS.iter().map(|s| s.to_string()).collect()),
            model: None,
            prompt: "You are a file search specialist. You are READ-ONLY: you cannot create, modify or delete files. Use Glob for broad file patterns, Grep for content, Read for specific files. Return absolute file paths and concise findings.".into(),
            source: "built-in".into(),
        },
        AgentDef {
            name: "Plan".into(),
            description: "Read-only software architect: explores the codebase and returns a step-by-step implementation plan, the critical files, and trade-offs.".into(),
            tools: Some(READ_TOOLS.iter().map(|s| s.to_string()).collect()),
            model: None,
            prompt: "You are a software architect. You are READ-ONLY. Explore the relevant code, then return a concrete implementation plan: ordered steps, the critical files (absolute paths) and the trade-offs you considered.".into(),
            source: "built-in".into(),
        },
    ]
}

fn parse(text: &str, fallback: &str, source: &str) -> Option<AgentDef> {
    let rest = text.strip_prefix("---")?;
    let end = rest.find("\n---")?;
    let fm = &rest[..end];
    let body = rest[end + 4..].trim().to_string();
    let mut name = fallback.to_string();
    let mut description = String::new();
    let mut tools = None;
    let mut model = None;
    for line in fm.lines() {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let v = v.trim().trim_matches('"').trim_matches('\'');
        match k.trim() {
            "name" if !v.is_empty() => name = v.to_string(),
            "description" => description = v.to_string(),
            "tools" if !v.is_empty() && v != "*" => {
                tools = Some(
                    v.split(',')
                        .map(|t| t.trim().to_string())
                        .filter(|t| !t.is_empty())
                        .collect(),
                )
            }
            "model" if !v.is_empty() && v != "inherit" => model = Some(v.to_string()),
            _ => {}
        }
    }
    Some(AgentDef {
        name,
        description,
        tools,
        model,
        prompt: body,
        source: source.into(),
    })
}

pub fn dirs(root: &Path) -> Vec<(PathBuf, &'static str)> {
    vec![
        (crate::settings::user_dir().join("agents"), "user"),
        (root.join(".claude/agents"), "project"),
        (root.join(".ice/agents"), "project"),
    ]
}

/// Built-ins, then custom agents (later definitions override by name).
pub fn all(root: &Path) -> Vec<AgentDef> {
    let mut out = builtins();
    for (dir, source) in dirs(root) {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut files: Vec<_> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("md"))
            .collect();
        files.sort();
        for p in files {
            let Ok(t) = std::fs::read_to_string(&p) else {
                continue;
            };
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("agent");
            if let Some(a) = parse(&t, stem, source) {
                out.retain(|x| !x.name.eq_ignore_ascii_case(&a.name));
                out.push(a);
            }
        }
    }
    out
}

pub fn find(root: &Path, name: &str) -> Option<AgentDef> {
    let all = all(root);
    let n = name.trim();
    all.iter()
        .find(|a| a.name.eq_ignore_ascii_case(n))
        .or_else(|| {
            let l = n.to_ascii_lowercase();
            all.iter().find(|a| {
                (l.contains("explor") || l.contains("search") || l.contains("find"))
                    && a.name == "Explore"
                    || l.contains("plan") && a.name == "Plan"
            })
        })
        .cloned()
}

/// Task tool description suffix listing the available agents.
pub fn listing(root: &Path) -> String {
    all(root)
        .iter()
        .map(|a| {
            let tools = a
                .tools
                .as_ref()
                .map(|t| t.join(", "))
                .unwrap_or_else(|| "*".into());
            format!("- {}: {} (Tools: {tools})", a.name, a.description)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn catalog(root: &Path) -> String {
    let mut s =
        String::from("Agents (use via the Task tool, or ask: \"use the <name> agent to …\"):\n");
    for a in all(root) {
        s.push_str(&format!(
            "  • {} [{}] — {}\n",
            a.name, a.source, a.description
        ));
    }
    s.push_str("\nCreate your own in .ice/agents/<name>.md with `name`, `description`, `tools` frontmatter.");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_agent_frontmatter() {
        let a = parse("---\nname: reviewer\ndescription: Reviews code\ntools: Read, Grep\nmodel: inherit\n---\nBe strict.", "x", "project").unwrap();
        assert_eq!(a.name, "reviewer");
        assert_eq!(a.tools.unwrap(), vec!["Read", "Grep"]);
        assert!(a.model.is_none());
        assert_eq!(a.prompt, "Be strict.");
    }

    #[test]
    fn fuzzy_builtin_lookup() {
        let root = std::env::temp_dir();
        assert_eq!(find(&root, "explore").unwrap().name, "Explore");
        assert_eq!(
            find(&root, "general-purpose").unwrap().name,
            "general-purpose"
        );
        assert!(find(&root, "nonexistent-agent").is_none());
    }
}
