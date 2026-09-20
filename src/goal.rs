use crate::ir::Assert;
use crate::verify::{self, AssertResult};
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

/// Durable outer-loop goal. Lives on disk so a crash/restart resumes.
#[derive(Debug, Clone)]
pub struct DurableGoal {
    pub text: String,
    pub done_when: Vec<Assert>,
    pub max_loops: u32,
    pub open: bool,
}

impl DurableGoal {
    pub fn new(text: impl Into<String>, done_when: Vec<Assert>, max_loops: u32) -> Self {
        Self {
            text: text.into(),
            done_when,
            max_loops: max_loops.max(1),
            open: true,
        }
    }

    pub fn dir(root: &Path) -> PathBuf {
        root.join(".ice")
    }

    pub fn path(root: &Path) -> PathBuf {
        Self::dir(root).join("goal.md")
    }

    pub fn progress_path(root: &Path) -> PathBuf {
        Self::dir(root).join("progress.md")
    }

    pub fn ensure_dir(root: &Path) -> Result<()> {
        fs::create_dir_all(Self::dir(root))?;
        fs::create_dir_all(Self::dir(root).join("agents"))?;
        Ok(())
    }

    pub fn save(&self, root: &Path) -> Result<()> {
        Self::ensure_dir(root)?;
        let mut body = String::new();
        body.push_str("# GOAL\n");
        body.push_str(&self.text);
        body.push('\n');
        body.push_str("\n# DONE WHEN\n");
        for a in &self.done_when {
            body.push_str(&format_assert(a));
            body.push('\n');
        }
        if self.done_when.is_empty() {
            body.push_str("exit 0\n");
        }
        body.push_str(&format!("\n# MAX LOOPS\n{}\n", self.max_loops));
        body.push_str(&format!(
            "\n# STATUS\n{}\n",
            if self.open { "open" } else { "done" }
        ));
        fs::write(Self::path(root), body)?;
        Ok(())
    }

    pub fn load(root: &Path) -> Option<Self> {
        let raw = fs::read_to_string(Self::path(root)).ok()?;
        Some(parse_goal_md(&raw))
    }

    pub fn clear(root: &Path) {
        let _ = fs::remove_file(Self::path(root));
        let _ = fs::remove_file(Self::progress_path(root));
    }

    pub fn satisfied(&self, root: &Path) -> Vec<AssertResult> {
        verify::check(root, &self.done_when, &[])
    }

    pub fn is_done(&self, root: &Path) -> bool {
        if self.done_when.is_empty() {
            return false;
        }
        verify::all_ok(&self.satisfied(root))
    }

    pub fn read_progress(root: &Path) -> String {
        fs::read_to_string(Self::progress_path(root)).unwrap_or_default()
    }

    pub fn append_progress(root: &Path, chunk: &str) -> Result<()> {
        Self::ensure_dir(root)?;
        let mut cur = Self::read_progress(root);
        if !cur.is_empty() && !cur.ends_with('\n') {
            cur.push('\n');
        }
        cur.push_str(chunk);
        if !chunk.ends_with('\n') {
            cur.push('\n');
        }
        fs::write(Self::progress_path(root), cur)?;
        Ok(())
    }

    pub fn reset_progress(root: &Path) -> Result<()> {
        Self::ensure_dir(root)?;
        fs::write(Self::progress_path(root), "# ICE progress\n")?;
        Ok(())
    }
}

fn format_assert(a: &Assert) -> String {
    match a {
        Assert::ExitZero => "exit 0".into(),
        Assert::ExitCode(c) => format!("exit {c}"),
        Assert::Contains { path, text } => format!("contains {path} {text}"),
        Assert::NotContains { path, text } => format!("not_contains {path} {text}"),
        Assert::FileExists { path } => format!("file_exists {path}"),
        Assert::FileNotExists { path } => format!("file_not_exists {path}"),
    }
}

fn parse_goal_md(raw: &str) -> DurableGoal {
    let mut text = String::new();
    let mut done_when = Vec::new();
    let mut max_loops = 8u32;
    let mut open = true;
    let mut sec = "goal";
    for line in raw.lines() {
        let t = line.trim();
        let u = t.to_ascii_uppercase();
        if u == "# GOAL" || u == "## GOAL" {
            sec = "goal";
            continue;
        }
        if u == "# DONE WHEN" || u == "## DONE WHEN" {
            sec = "done";
            continue;
        }
        if u == "# MAX LOOPS" || u == "## MAX LOOPS" {
            sec = "max";
            continue;
        }
        if u == "# STATUS" || u == "## STATUS" {
            sec = "status";
            continue;
        }
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        match sec {
            "goal" => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(t);
            }
            "done" => {
                if let Ok(a) = crate::ir::parse_assert_line(t) {
                    done_when.push(a);
                }
            }
            "max" => {
                if let Ok(n) = t.parse::<u32>() {
                    max_loops = n.max(1);
                }
            }
            "status" => open = !t.eq_ignore_ascii_case("done"),
            _ => {}
        }
    }
    if done_when.is_empty() {
        done_when.push(Assert::FileExists {
            path: ".ice/DONE".into(),
        });
    }
    DurableGoal {
        text,
        done_when,
        max_loops,
        open,
    }
}

/// Parse `/goal fix tests until file_exists .ice/ok` style argument.
pub fn parse_goal_arg(arg: &str) -> (String, Vec<Assert>, u32) {
    let (text, rest) = split_until(arg);
    let mut done = Vec::new();
    let mut max = 8u32;
    for part in rest.split(';') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        if let Some(n) = p.strip_prefix("loops ") {
            if let Ok(v) = n.trim().parse::<u32>() {
                max = v;
                continue;
            }
        }
        if let Ok(a) = crate::ir::parse_assert_line(p) {
            done.push(a);
        }
    }
    if done.is_empty() {
        done.push(Assert::FileExists {
            path: ".ice/DONE".into(),
        });
    }
    (text, done, max)
}

fn split_until(arg: &str) -> (String, String) {
    let lower = arg.to_ascii_lowercase();
    if let Some(i) = lower.find(" until ") {
        return (arg[..i].trim().to_string(), arg[i + 7..].trim().to_string());
    }
    (arg.trim().to_string(), String::new())
}
