//! `ice init`: project scaffolding — shared settings with sensible
//! permission rules, and folders for custom commands and agents. Existing
//! files are never overwritten. (The interactive `/init` additionally has
//! the model write an ICE.md describing the codebase.)

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

const SETTINGS: &str = r#"{
  "permissions": {
    "allow": [
      "Bash(git status:*)",
      "Bash(git diff:*)",
      "Bash(git log:*)"
    ],
    "deny": [
      "Read(./.env)",
      "Read(./.env.*)",
      "Read(./secrets/**)"
    ]
  }
}
"#;

const COMMAND_EXAMPLE: &str = r#"---
description: Explain a file or symbol in plain language
---
Explain $ARGUMENTS: what it does, how it fits into the codebase, and anything surprising. Reference code as `file:line`.
"#;

const AGENT_EXAMPLE: &str = r#"---
name: reviewer
description: Reviews recent changes for bugs, risky patterns and missing tests. Use after writing code.
tools: Read, Grep, Glob, Bash
---
You are a meticulous senior code reviewer. Run `git diff` to see recent changes, read the surrounding code, and report concrete problems ordered by severity, each with a file:line reference and a suggested fix. Do not modify files.
"#;

const GITIGNORE: &str = "settings.local.json\n";

pub fn ensure(root: &Path) -> Result<Vec<PathBuf>> {
    let ice = root.join(".ice");
    let mut created = Vec::new();
    for (rel, body) in [
        ("settings.json", SETTINGS),
        (".gitignore", GITIGNORE),
        ("commands/explain.md", COMMAND_EXAMPLE),
        ("agents/reviewer.md", AGENT_EXAMPLE),
    ] {
        let p = ice.join(rel);
        if p.exists() {
            continue;
        }
        if let Some(d) = p.parent() {
            fs::create_dir_all(d).with_context(|| format!("mkdir {}", d.display()))?;
        }
        fs::write(&p, body).with_context(|| format!("write {}", p.display()))?;
        created.push(p);
    }
    Ok(created)
}

pub fn describe(root: &Path, created: &[PathBuf]) -> String {
    let mut s = format!("ICE project in {}\n", root.display());
    if created.is_empty() {
        s.push_str("Everything was already in place.\n");
    }
    for p in created {
        s.push_str(&format!(
            "  created {}\n",
            p.strip_prefix(root).unwrap_or(p).display()
        ));
    }
    s.push_str("\nNext: run `ice` here and type /init to generate ICE.md (project instructions for the agent).");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_is_idempotent() {
        let d = std::env::temp_dir().join(format!("ice-init-{}", crate::conv::new_id("i")));
        let first = ensure(&d).unwrap();
        assert_eq!(first.len(), 4);
        assert!(ensure(&d).unwrap().is_empty());
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(d.join(".ice/settings.json")).unwrap())
                .unwrap();
        assert!(v["permissions"]["deny"].is_array());
    }
}
