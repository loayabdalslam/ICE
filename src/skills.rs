//! Skills: `SKILL.md` instruction packs with YAML-ish frontmatter
//! (`name`, `description`). Discovered in `.ice/skills/<name>/`,
//! `.claude/skills/<name>/` (compatible layout) and `~/.ice/skills/<name>/`.
//! Only the name + description go into the tool list; the body is loaded on
//! demand by the Skill tool, keeping the context small.

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub body: String,
}

fn parse(text: &str, fallback_name: &str) -> (String, String, String) {
    let mut name = fallback_name.to_string();
    let mut desc = String::new();
    let mut body = text;
    if let Some(rest) = text.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            let fm = &rest[..end];
            body = rest[end + 4..].trim_start_matches(['\r', '\n']);
            for line in fm.lines() {
                if let Some((k, v)) = line.split_once(':') {
                    let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
                    match k.trim() {
                        "name" if !v.is_empty() => name = v,
                        "description" => desc = v,
                        _ => {}
                    }
                }
            }
        }
    }
    if desc.is_empty() {
        desc = body
            .lines()
            .map(|l| l.trim().trim_start_matches('#').trim())
            .find(|l| !l.is_empty())
            .unwrap_or("")
            .chars()
            .take(160)
            .collect();
    }
    (name, desc, body.to_string())
}

pub fn dirs(root: &Path) -> Vec<PathBuf> {
    vec![
        root.join(".ice/skills"),
        root.join(".claude/skills"),
        crate::settings::user_dir().join("skills"),
    ]
}

pub fn discover(root: &Path) -> Vec<Skill> {
    let mut out: Vec<Skill> = Vec::new();
    for dir in dirs(root) {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            let (md, fallback) = if p.is_dir() {
                (
                    p.join("SKILL.md"),
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("skill")
                        .to_string(),
                )
            } else if p.extension().and_then(|x| x.to_str()) == Some("md") {
                (
                    p.clone(),
                    p.file_stem()
                        .and_then(|n| n.to_str())
                        .unwrap_or("skill")
                        .to_string(),
                )
            } else {
                continue;
            };
            let Ok(text) = fs::read_to_string(&md) else {
                continue;
            };
            let (name, description, body) = parse(&text, &fallback);
            if !out.iter().any(|s| s.name == name) {
                out.push(Skill {
                    name,
                    description,
                    path: md,
                    body,
                });
            }
        }
    }
    out
}

/// "- name: description" lines for the Skill tool description.
pub fn listing(root: &Path) -> Vec<String> {
    discover(root)
        .into_iter()
        .map(|s| format!("- {}: {}", s.name, s.description))
        .collect()
}

pub fn catalog(root: &Path) -> String {
    let skills = discover(root);
    if skills.is_empty() {
        return "No skills found. Create .ice/skills/<name>/SKILL.md with a `name` and `description` frontmatter.".into();
    }
    let mut s = format!(
        "{} skill{}:\n",
        skills.len(),
        if skills.len() == 1 { "" } else { "s" }
    );
    for sk in skills {
        s.push_str(&format!(
            "  • {} — {}\n    {}\n",
            sk.name,
            sk.description,
            sk.path.display()
        ));
    }
    s
}

pub fn load_named(root: &Path, name: &str) -> Option<Skill> {
    discover(root)
        .into_iter()
        .find(|s| s.name.eq_ignore_ascii_case(name.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_is_parsed() {
        let (n, d, b) = parse(
            "---\nname: pdf\ndescription: \"Work with PDFs\"\n---\n# PDF\nsteps",
            "x",
        );
        assert_eq!((n.as_str(), d.as_str()), ("pdf", "Work with PDFs"));
        assert!(b.starts_with("# PDF"));
        let (n, d, _) = parse("# Deploy helper\nbody", "deploy");
        assert_eq!((n.as_str(), d.as_str()), ("deploy", "Deploy helper"));
    }
}
