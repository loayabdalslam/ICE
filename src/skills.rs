//! Skills: SKILL.md files loaded into the compiler context.

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Skill {
    pub name: String,
    pub path: PathBuf,
    pub body: String,
}

pub fn discover(root: &Path) -> Vec<Skill> {
    let mut dirs = vec![root.join(".ice/skills"), root.join("skills")];
    if let Ok(h) = std::env::var("HOME") {
        dirs.push(PathBuf::from(h).join(".ice/skills"));
    }
    dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("skills"));

    let mut out = Vec::new();
    for dir in dirs {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let skill_md = if p.is_dir() {
                p.join("SKILL.md")
            } else if p.file_name().and_then(|n| n.to_str()) == Some("SKILL.md") {
                p
            } else if p.extension().and_then(|x| x.to_str()) == Some("md") {
                p
            } else {
                continue;
            };
            if !skill_md.is_file() {
                continue;
            }
            let name = skill_md
                .parent()
                .and_then(|d| d.file_name())
                .and_then(|n| n.to_str())
                .filter(|n| *n != "skills")
                .unwrap_or("skill")
                .to_string();
            if let Ok(body) = fs::read_to_string(&skill_md) {
                if !out.iter().any(|s: &Skill| s.name == name) {
                    out.push(Skill {
                        name,
                        path: skill_md,
                        body,
                    });
                }
            }
        }
    }
    out
}

pub fn catalog(root: &Path) -> String {
    let skills = discover(root);
    if skills.is_empty() {
        return "no skills loaded. drop SKILL.md under .ice/skills/<name>/".into();
    }
    let mut s = format!("{} skill(s):\n", skills.len());
    for sk in skills {
        let first = sk.body.lines().next().unwrap_or("").trim();
        s.push_str(&format!("  • {}  {}\n", sk.name, first));
    }
    s
}

pub fn prompt_block(root: &Path) -> String {
    let skills = discover(root);
    if skills.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n# Installed skills\nUse `skill <name>` to apply one.\n");
    for sk in skills {
        let excerpt: String = sk.body.chars().take(900).collect();
        s.push_str(&format!("\n## skill:{}\n{}\n", sk.name, excerpt));
    }
    s
}

pub fn load_named(root: &Path, name: &str) -> Option<Skill> {
    discover(root)
        .into_iter()
        .find(|s| s.name.eq_ignore_ascii_case(name))
}
