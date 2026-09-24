//! System prompt assembly: behaviour guide, environment, git snapshot and
//! project memory (ICE.md / CLAUDE.md / AGENTS.md, with `@path` imports).

use crate::tools::home_dir;
use std::path::{Path, PathBuf};
use std::process::Command;

const BASE: &str = r#"You are ICE, an interactive command-line coding agent. You help users with software engineering tasks: fixing bugs, adding features, refactoring, explaining code, running and fixing tests, and more. Use the instructions below and the tools available to you to assist the user.

IMPORTANT: Refuse to write or improve code that is clearly intended to be malicious, even if the user says it is for education. Defensive security work (analysis, detection rules, vulnerability explanations) is fine.
IMPORTANT: Never generate or guess URLs unless you are confident they help the user with programming. You may use URLs the user provides or that appear in local files.

# Tone and style
- Be concise, direct and to the point. Your output is shown in a terminal and rendered as GitHub-flavored markdown in a monospace font.
- Keep responses short: fewer than 4 lines of prose (not counting tool calls or generated code) unless the user asks for detail. One-word answers are best when they suffice.
- Do not add preamble or postamble such as "Here is what I did" or "Let me know if you need anything else" unless asked. After working on a file, just stop.
- Explain non-trivial shell commands you run that change the user's system, and why.
- Communicate with the user only through your text output; never use tools like Bash or code comments to talk to the user.
- Answer in the language the user writes in.
- Only use emojis if the user explicitly asks for them.
- If you cannot or will not help with something, say so in 1-2 sentences without moralising, and offer an alternative if you can.

# Proactiveness
Do what the user asks — including reasonable follow-up actions — without surprising them. If the user asks how to approach something, answer the question first instead of jumping into changes.

# Following conventions
When changing files, first understand the file's conventions. Mimic the code style, use existing libraries and utilities, and follow existing patterns.
- NEVER assume a library is available; check package manifests and neighbouring files first.
- When creating a component, look at existing ones for framework choice, naming and typing.
- When editing, look at the surrounding context (especially imports) to make the change idiomatic.
- Follow security best practices. Never introduce code that exposes or logs secrets and keys, and never commit secrets.
- Do not add comments to the code you write unless asked or the code is genuinely complex.

# Task management
You have the TodoWrite tool. Use it VERY frequently for multi-step tasks, to plan the work and show the user your progress. Mark a todo as completed as soon as it is done — do not batch several completions. Keep exactly one todo in_progress at a time.

# Doing tasks
1. Use the search tools (Glob, Grep, LS, Read — in parallel when independent) to understand the codebase and the request.
2. Implement the solution using all tools available to you.
3. Verify the solution with the project's tests if possible. Never assume a test framework; check the README or search the codebase.
4. When you finish, run the project's lint and typecheck commands (e.g. npm run lint, cargo clippy, ruff) if you know them. If you can't find them, ask the user and suggest saving them to ICE.md.
NEVER commit changes unless the user explicitly asks you to.

# Tool usage policy
- Prefer the Task tool for open-ended searches that may take several rounds, to keep your context small.
- You can call multiple tools in a single response. When several independent pieces of information are needed, batch the tool calls together — e.g. run `git status` and `git diff` in parallel.
- Use dedicated tools rather than shell equivalents: Read (not cat/head/tail), Edit (not sed/awk), Write (not echo/heredoc), Glob (not find/ls), Grep (not grep/rg).
- When WebFetch reports a redirect to another host, fetch the new URL.
- A tool call may be denied by the user. If so, do not retry the same call; adjust your approach or ask the user what they want.

# Code references
When referring to specific code, use the pattern `file_path:line_number` so the user can navigate to it.

# Safety
The working tree is the user's real project. Prefer reversible actions. Before deleting or overwriting, look at the target. Never run destructive git commands (reset --hard, push --force, clean -fd) unless the user asks."#;

/// A memory file found for this project.
#[derive(Clone, Debug)]
pub struct MemoryFile {
    pub path: PathBuf,
    pub scope: &'static str,
    pub content: String,
}

pub const MEMORY_NAMES: &[&str] = &["ICE.md", "CLAUDE.md", "AGENTS.md"];

/// User memory (~/.ice/ICE.md), then every ICE.md/CLAUDE.md/AGENTS.md from
/// the filesystem root down to the project, then ICE.local.md.
pub fn load_memory(root: &Path) -> Vec<MemoryFile> {
    let mut out = Vec::new();
    let user = crate::settings::user_dir().join("ICE.md");
    if let Ok(c) = std::fs::read_to_string(&user) {
        out.push(MemoryFile {
            content: expand_imports(&c, user.parent().unwrap_or(Path::new(".")), 0),
            path: user,
            scope: "user",
        });
    }
    let mut dirs: Vec<PathBuf> = root.ancestors().map(Path::to_path_buf).collect();
    dirs.reverse();
    let home = home_dir();
    for d in dirs {
        // Skip the home directory itself unless it is the project.
        if d == home && d != root {
            continue;
        }
        for name in MEMORY_NAMES
            .iter()
            .chain(["ICE.local.md", ".ice/ICE.md"].iter())
        {
            let p = d.join(name);
            if let Ok(c) = std::fs::read_to_string(&p) {
                if out.iter().any(|m: &MemoryFile| m.content == c) {
                    continue; // AGENTS.md symlinked to CLAUDE.md, etc.
                }
                let scope = if name.contains("local") {
                    "local"
                } else if d == root {
                    "project"
                } else {
                    "parent"
                };
                out.push(MemoryFile {
                    content: expand_imports(&c, &d, 0),
                    path: p,
                    scope,
                });
            }
        }
    }
    out
}

/// `@path/to/file` lines pull that file in (max depth 5), outside code fences.
fn expand_imports(text: &str, base: &Path, depth: u32) -> String {
    if depth >= 5 {
        return text.to_string();
    }
    let mut out = String::new();
    let mut fence = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("```") {
            fence = !fence;
        }
        if !fence && t.starts_with('@') && !t.contains(' ') && t.len() > 1 {
            let rel = &t[1..];
            let p = if let Some(r) = rel.strip_prefix("~/") {
                home_dir().join(r)
            } else {
                base.join(rel)
            };
            if let Ok(c) = std::fs::read_to_string(&p) {
                out.push_str(&expand_imports(&c, p.parent().unwrap_or(base), depth + 1));
                out.push('\n');
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let o = Command::new("git")
        .args(args)
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !o.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&o.stdout).trim_end().to_string())
}

pub fn is_git_repo(root: &Path) -> bool {
    git(root, &["rev-parse", "--is-inside-work-tree"])
        .map(|s| s == "true")
        .unwrap_or(false)
}

/// Snapshot of git state at conversation start.
pub fn git_status(root: &Path) -> Option<String> {
    if !is_git_repo(root) {
        return None;
    }
    let branch = git(root, &["branch", "--show-current"]).unwrap_or_default();
    let main = git(
        root,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .map(|s| s.trim_start_matches("origin/").to_string())
    .unwrap_or_else(|| {
        if git(root, &["rev-parse", "--verify", "--quiet", "main"]).is_some() {
            "main".into()
        } else {
            "master".into()
        }
    });
    let mut status = git(root, &["status", "--short"]).unwrap_or_default();
    if status.len() > 2000 {
        status = format!(
            "{}\n... (truncated; run \"git status\" for the full list)",
            &status[..status
                .char_indices()
                .take_while(|(i, _)| *i < 2000)
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0)]
        );
    }
    let log = git(root, &["log", "--oneline", "-n", "5"]).unwrap_or_default();
    Some(format!(
        "gitStatus: This is the git status at the start of the conversation. It is a snapshot and will not update during the conversation.\nCurrent branch: {branch}\n\nMain branch (you will usually use this for PRs): {main}\n\nStatus:\n{}\n\nRecent commits:\n{log}",
        if status.trim().is_empty() { "(clean)".to_string() } else { status }
    ))
}

pub fn today() -> String {
    // Civil date from the unix clock (UTC), no chrono dependency.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// Howard Hinnant's days→civil algorithm.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn os_version() -> String {
    if cfg!(target_os = "linux") {
        Command::new("uname")
            .arg("-sr")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|| "Linux".into())
    } else if cfg!(target_os = "macos") {
        Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .map(|o| format!("macOS {}", String::from_utf8_lossy(&o.stdout).trim()))
            .unwrap_or_else(|| "macOS".into())
    } else {
        std::env::consts::OS.to_string()
    }
}

pub struct PromptParts<'a> {
    pub root: &'a Path,
    pub model: &'a str,
    pub memory: &'a [MemoryFile],
    pub git_status: Option<&'a str>,
    pub append: Option<&'a str>,
    pub replace: Option<&'a str>,
    pub subagent: Option<&'a str>,
}

pub fn system_prompt(p: &PromptParts) -> String {
    let mut s = match (p.replace, p.subagent) {
        (Some(r), _) => r.to_string(),
        (None, Some(role)) => format!(
            "You are an agent for ICE, a command-line coding agent. {role}\n\nDo what has been asked; nothing more, nothing less. When you complete the task, respond with a detailed writeup: the final response is returned to the main agent, not shown to the user, so include file paths (absolute) and relevant code snippets. Do not use emojis. Prefer the search tools; read files in parallel when useful."
        ),
        (None, None) => BASE.to_string(),
    };
    s.push_str(&format!(
        "\n\nHere is useful information about the environment you are running in:\n<env>\nWorking directory: {}\nIs directory a git repo: {}\nPlatform: {}\nOS Version: {}\nToday's date: {}\n</env>\nYou are powered by the model {}.",
        p.root.display(),
        if is_git_repo(p.root) { "Yes" } else { "No" },
        std::env::consts::OS,
        os_version(),
        today(),
        p.model
    ));
    if let Some(g) = p.git_status {
        s.push_str("\n\n");
        s.push_str(g);
    }
    if !p.memory.is_empty() {
        s.push_str("\n\n# Project and user instructions\nCodebase and user instructions are shown below. Be sure to adhere to these instructions. IMPORTANT: These instructions OVERRIDE any default behavior and you MUST follow them exactly as written.\n");
        for m in p.memory {
            let label = match m.scope {
                "user" => "user's private global instructions for all projects",
                "local" => "user's private project instructions, not checked in",
                _ => "project instructions, checked into the codebase",
            };
            s.push_str(&format!(
                "\nContents of {} ({label}):\n\n{}\n",
                m.path.display(),
                m.content.trim()
            ));
        }
    }
    if let Some(a) = p.append {
        s.push_str("\n\n");
        s.push_str(a);
    }
    s
}

pub const PLAN_MODE_REMINDER: &str = "<system-reminder>\nPlan mode is active. The user indicated that they do not want you to execute yet -- you MUST NOT make any edits, run any non-readonly tools (including changing configs or making commits), or otherwise make any changes to the system. This supersedes any other instructions you have received. Instead you should:\n1. Answer the user's query comprehensively, using read-only tools to research.\n2. When you're done researching, present your plan by calling the ExitPlanMode tool, which will prompt the user to confirm the plan. Do NOT make any file changes or run any tools that modify the system state in any way until the user has confirmed the plan.\n</system-reminder>";

/// Extended-thinking budget from the user's wording, as in Claude Code:
/// "think" < "think hard" < "think harder" < "ultrathink".
pub fn thinking_budget(prompt: &str) -> u32 {
    let p = prompt.to_lowercase();
    if p.contains("ultrathink")
        || p.contains("think harder")
        || p.contains("think intensely")
        || p.contains("think really hard")
        || p.contains("think super hard")
    {
        31_999
    } else if p.contains("megathink")
        || p.contains("think hard")
        || p.contains("think deeply")
        || p.contains("think a lot")
        || p.contains("think more")
    {
        10_000
    } else if regex::Regex::new(r"\bthink\b")
        .map(|r| r.is_match(&p))
        .unwrap_or(false)
    {
        4_000
    } else {
        0
    }
}

pub const INIT_PROMPT: &str = "Please analyze this codebase and create an ICE.md file, which will be given to future instances of ICE to operate in this repository.\n\nWhat to add:\n1. Commands that will be commonly used, such as how to build, lint, and run tests. Include the necessary commands to develop in this codebase, such as how to run a single test.\n2. High-level code architecture and structure so that future instances can be productive more quickly. Focus on the \"big picture\" architecture that requires reading multiple files to understand.\n\nUsage notes:\n- If there's already an ICE.md (or CLAUDE.md / AGENTS.md), suggest improvements to it.\n- Do not repeat yourself and do not include obvious instructions like \"Provide helpful error messages\" or \"Write unit tests for all new utilities\".\n- Avoid listing every component or file structure that can be easily discovered.\n- Don't include generic development practices.\n- If there are Cursor rules (.cursor/rules/ or .cursorrules) or Copilot rules (.github/copilot-instructions.md), include the important parts.\n- If there is a README.md, include the important parts.\n- Do not make up information such as \"Common Development Tasks\" unless it is expressly included in other files you read.\n- Prefix the file with the following text:\n\n```\n# ICE.md\n\nThis file provides guidance to ICE when working with code in this repository.\n```";

pub const COMPACT_PROMPT: &str = "Your task is to create a detailed summary of the conversation so far, paying close attention to the user's explicit requests and your previous actions. This summary will replace the conversation history, so it must capture everything needed to continue the work without losing context.\n\nInclude these sections:\n1. Primary Request and Intent: all of the user's explicit requests and intents in detail.\n2. Key Technical Concepts: technologies, frameworks and concepts discussed.\n3. Files and Code Sections: files examined, modified or created, with important code snippets and why they matter.\n4. Errors and fixes: errors you ran into and how you fixed them, including user feedback.\n5. Problem Solving: problems solved and ongoing troubleshooting.\n6. All user messages: every non-tool-result user message, verbatim or nearly so.\n7. Pending Tasks: tasks you have explicitly been asked to work on.\n8. Current Work: precisely what was being worked on immediately before this summary, with file names and snippets.\n9. Optional Next Step: the next step directly in line with the most recent request, quoting the most recent conversation to show exactly where you left off.\n\nRespond with the summary only.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_keywords_set_budgets() {
        assert_eq!(thinking_budget("fix it"), 0);
        assert_eq!(thinking_budget("think about the design"), 4_000);
        assert_eq!(thinking_budget("think hard about this"), 10_000);
        assert_eq!(thinking_budget("ultrathink: redesign"), 31_999);
        assert_eq!(thinking_budget("rethinking is fine"), 0);
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_720), (2026, 9, 24));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn memory_imports_expand() {
        let d = std::env::temp_dir().join(format!("ice-mem-{}", crate::conv::new_id("m")));
        std::fs::create_dir_all(d.join("docs")).unwrap();
        std::fs::write(d.join("docs/style.md"), "Use tabs.").unwrap();
        std::fs::write(
            d.join("ICE.md"),
            "# Rules\n@docs/style.md\n```\n@not/imported\n```\n",
        )
        .unwrap();
        let mem = load_memory(&d);
        let m = mem.iter().find(|m| m.path.ends_with("ICE.md")).unwrap();
        assert!(m.content.contains("Use tabs."));
        assert!(m.content.contains("@not/imported"));
        let sp = system_prompt(&PromptParts {
            root: &d,
            model: "m",
            memory: &mem,
            git_status: None,
            append: Some("EXTRA"),
            replace: None,
            subagent: None,
        });
        assert!(sp.contains("Use tabs.") && sp.ends_with("EXTRA"));
        assert!(sp.contains("<env>"));
    }
}
