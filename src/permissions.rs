//! Permission modes and rules, modelled on Claude Code.
//!
//! Modes: default (ask before edits and non-trivial commands), acceptEdits
//! (edits inside the project auto-approved), plan (read-only research until
//! the plan is approved), bypassPermissions (never ask).
//!
//! Rules: `Tool`, `Tool(specifier)`. Bash specifiers are exact commands or
//! prefixes ending in `:*`; file tools take gitignore-style globs; WebFetch
//! takes `domain:host`. Deny beats ask beats allow.

use crate::tools::{self, ToolCtx};
use serde_json::Value;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Default,
    AcceptEdits,
    Plan,
    BypassPermissions,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim() {
            "default" => Some(Mode::Default),
            "acceptEdits" | "accept-edits" | "accept_edits" => Some(Mode::AcceptEdits),
            "plan" => Some(Mode::Plan),
            "bypassPermissions" | "bypass" | "yolo" => Some(Mode::BypassPermissions),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Mode::Default => "default",
            Mode::AcceptEdits => "acceptEdits",
            Mode::Plan => "plan",
            Mode::BypassPermissions => "bypassPermissions",
        }
    }

    /// Shift+Tab cycle: default → acceptEdits → plan → default. (Bypass is
    /// only reachable from the command line, as in Claude Code.)
    pub fn cycle(self, bypass_available: bool) -> Mode {
        match self {
            Mode::Default => Mode::AcceptEdits,
            Mode::AcceptEdits => Mode::Plan,
            Mode::Plan if bypass_available => Mode::BypassPermissions,
            _ => Mode::Default,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Ask,
    Deny(String),
}

#[derive(Clone, Debug, Default)]
pub struct Rules {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub ask: Vec<String>,
}

fn parse_rule(r: &str) -> (&str, Option<&str>) {
    let r = r.trim();
    match r.find('(') {
        Some(i) if r.ends_with(')') => (&r[..i], Some(&r[i + 1..r.len() - 1])),
        _ => (r, None),
    }
}

fn tool_matches(rule_tool: &str, tool: &str) -> bool {
    if rule_tool == tool {
        return true;
    }
    // "Edit" covers every file-editing tool.
    if rule_tool == "Edit" && tools::EDIT_TOOLS.contains(&tool) {
        return true;
    }
    // mcp__server matches every tool on that server.
    tool.starts_with("mcp__") && tool.starts_with(&format!("{rule_tool}__"))
}

/// Split a shell command line into simple commands on && || ; | and newlines,
/// ignoring separators inside quotes.
pub fn split_commands(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut sq, mut dq) = (false, false);
    let chars: Vec<char> = cmd.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' if !dq => sq = !sq,
            '"' if !sq => dq = !dq,
            _ => {}
        }
        if !sq && !dq {
            let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
            if two == "&&" || two == "||" {
                out.push(std::mem::take(&mut cur));
                i += 2;
                continue;
            }
            if c == ';' || c == '|' || c == '\n' || (c == '&' && two != "&>") {
                out.push(std::mem::take(&mut cur));
                i += 1;
                continue;
            }
        }
        cur.push(c);
        i += 1;
    }
    out.push(cur);
    out.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn bash_rule_matches(spec: &str, cmd: &str) -> bool {
    let cmd = cmd.trim();
    if let Some(prefix) = spec.strip_suffix(":*") {
        let prefix = prefix.trim();
        cmd == prefix || cmd.starts_with(&format!("{prefix} "))
    } else if let Some(prefix) = spec.strip_suffix('*') {
        cmd.starts_with(prefix.trim_end())
    } else {
        cmd == spec.trim()
    }
}

fn path_rule_matches(spec: &str, path: &Path, root: &Path) -> bool {
    let spec = spec.trim();
    let (base, pat) = if let Some(rest) = spec.strip_prefix("~/") {
        (tools::home_dir(), rest.to_string())
    } else if let Some(rest) = spec.strip_prefix("//") {
        (std::path::PathBuf::from("/"), rest.to_string())
    } else if let Some(rest) = spec.strip_prefix("./") {
        (root.to_path_buf(), rest.to_string())
    } else if let Some(rest) = spec.strip_prefix('/') {
        (root.to_path_buf(), rest.to_string())
    } else {
        (root.to_path_buf(), format!("**/{spec}"))
    };
    let rel = match tools::normalize(path).strip_prefix(tools::normalize(&base)) {
        Ok(r) => r.to_path_buf(),
        Err(_) => return false,
    };
    globset::Glob::new(&pat)
        .map(|g| g.compile_matcher().is_match(&rel))
        .unwrap_or(false)
}

impl Rules {
    /// `any_part`: for deny/ask, a compound Bash command hits when ANY part
    /// matches; for allow, every part must.
    fn rule_hits(
        &self,
        list: &[String],
        tool: &str,
        input: &Value,
        ctx: &ToolCtx,
        any_part: bool,
    ) -> bool {
        list.iter().any(|r| {
            let (rt, spec) = parse_rule(r);
            if !tool_matches(rt, tool) {
                return false;
            }
            let Some(spec) = spec else { return true };
            match tool {
                "Bash" => {
                    let cmd = input["command"].as_str().unwrap_or("");
                    // A compound command only matches if every part does.
                    let parts = split_commands(cmd);
                    if any_part {
                        parts.iter().any(|p| bash_rule_matches(spec, p))
                            || bash_rule_matches(spec, cmd)
                    } else {
                        // Never match a prefix rule against a whole compound
                        // command: `npm test && rm -rf x` must not pass `npm test:*`.
                        !parts.is_empty() && parts.iter().all(|p| bash_rule_matches(spec, p))
                    }
                }
                "WebFetch" => {
                    let host = tools::host_of(input["url"].as_str().unwrap_or(""));
                    spec.strip_prefix("domain:")
                        .map(|d| host == d || host.ends_with(&format!(".{d}")))
                        .unwrap_or(false)
                }
                _ => match target_path(tool, input) {
                    Some(p) => path_rule_matches(spec, &ctx.resolve(&p), &ctx.root),
                    None => false,
                },
            }
        })
    }

    /// Same as rule_hits but for Bash compound commands: each sub-command may
    /// be allowed by a different rule (`git add:*` and `git commit:*`).
    fn bash_all_allowed(&self, cmd: &str) -> bool {
        let parts = split_commands(cmd);
        !parts.is_empty()
            && parts.iter().all(|p| {
                self.allow.iter().any(|r| {
                    let (rt, spec) = parse_rule(r);
                    rt == "Bash" && spec.map(|s| bash_rule_matches(s, p)).unwrap_or(true)
                }) || is_safe_command(p)
            })
    }
}

/// The path a file tool is about to touch.
pub fn target_path(tool: &str, input: &Value) -> Option<String> {
    match tool {
        "Read" | "Write" | "Edit" | "MultiEdit" => input["file_path"].as_str().map(String::from),
        "Glob" | "Grep" | "LS" => Some(input["path"].as_str().unwrap_or(".").to_string()),
        _ => None,
    }
}

/// Read-only commands that never need approval (no redirection, no
/// substitution, no destructive flags).
pub fn is_safe_command(cmd: &str) -> bool {
    let c = cmd.trim();
    if c.is_empty() {
        return true;
    }
    if c.contains('>') || c.contains("$(") || c.contains('`') || c.contains("<(") {
        return false;
    }
    let words: Vec<&str> = c.split_whitespace().collect();
    let first = words[0];
    const SAFE: &[&str] = &[
        "ls", "pwd", "cat", "head", "tail", "wc", "echo", "printf", "grep", "rg", "which",
        "whoami", "date", "file", "stat", "du", "df", "tree", "sort", "uniq", "cut", "basename",
        "dirname", "realpath", "true", "false", "uname", "env", "printenv", "id", "hostname",
        "diff", "cmp", "nl", "less", "more", "type", "test", "[", "jq",
    ];
    if SAFE.contains(&first) {
        return !(first == "env" && words.len() > 1 && !words[1].starts_with('-'));
    }
    if first == "find" {
        return !words.iter().any(|w| {
            matches!(
                *w,
                "-exec" | "-execdir" | "-delete" | "-ok" | "-fprint" | "-fls"
            )
        });
    }
    if first == "git" {
        let sub = words.get(1).copied().unwrap_or("");
        return match sub {
            "status" | "diff" | "log" | "show" | "blame" | "rev-parse" | "ls-files"
            | "describe" | "shortlog" | "reflog" => true,
            "branch" => !words.iter().any(|w| {
                matches!(
                    *w,
                    "-d" | "-D" | "-m" | "-M" | "--delete" | "--move" | "-c" | "-C"
                )
            }),
            "remote" => words.len() == 2 || words.get(2) == Some(&"-v"),
            "config" => words
                .iter()
                .any(|w| *w == "--get" || *w == "--list" || *w == "-l"),
            "tag" => {
                words.len() == 2 || words.get(2) == Some(&"-l") || words.get(2) == Some(&"--list")
            }
            "stash" => words.get(2) == Some(&"list"),
            _ => false,
        };
    }
    if matches!(
        first,
        "cargo" | "npm" | "node" | "python" | "python3" | "go" | "rustc" | "java" | "ruby"
    ) && words.len() == 2
        && matches!(words[1], "--version" | "-V" | "-v" | "version")
    {
        return true;
    }
    false
}

/// Filesystem commands auto-approved in acceptEdits mode.
fn is_fs_command(cmd: &str) -> bool {
    let first = cmd.split_whitespace().next().unwrap_or("");
    matches!(first, "mkdir" | "touch" | "cp" | "mv")
}

/// Suggested "don't ask again" rule for a Bash command: `npm run test:*`.
pub fn suggest_bash_rule(cmd: &str) -> String {
    let first_cmd = split_commands(cmd).into_iter().next().unwrap_or_default();
    let words: Vec<&str> = first_cmd.split_whitespace().collect();
    if words.is_empty() {
        return "Bash".into();
    }
    const TWO_WORD: &[&str] = &[
        "git", "npm", "pnpm", "yarn", "bun", "cargo", "go", "docker", "kubectl", "pip", "pip3",
        "uv", "poetry", "make", "dotnet", "gradle", "mvn", "brew", "apt", "gh", "npx", "deno",
        "swift", "mix", "bundle", "rails",
    ];
    let n = if TWO_WORD.contains(&words[0]) && words.len() > 1 && !words[1].starts_with('-') {
        if (words[0] == "npm" || words[0] == "pnpm" || words[0] == "yarn" || words[0] == "bun")
            && words[1] == "run"
            && words.len() > 2
        {
            3
        } else {
            2
        }
    } else {
        1
    };
    format!("Bash({}:*)", words[..n].join(" "))
}

pub struct Checker<'a> {
    pub rules: &'a Rules,
    pub mode: Mode,
}

impl<'a> Checker<'a> {
    pub fn check(&self, tool: &str, input: &Value, ctx: &ToolCtx) -> Decision {
        let r = self.rules;
        if r.rule_hits(&r.deny, tool, input, ctx, true) {
            return Decision::Deny(format!(
                "Permission to use {tool} has been denied by a permissions rule."
            ));
        }
        if self.mode == Mode::BypassPermissions {
            return Decision::Allow;
        }
        if self.mode == Mode::Plan && !tools::is_read_only(tool) {
            return Decision::Deny(
                "Plan mode is active: you may only use read-only tools. Research the task, then call ExitPlanMode with your plan for the user to approve before making changes.".into(),
            );
        }
        if r.rule_hits(&r.ask, tool, input, ctx, true) {
            return Decision::Ask;
        }
        if r.rule_hits(&r.allow, tool, input, ctx, false) {
            return Decision::Allow;
        }
        match tool {
            "Read" | "Glob" | "Grep" | "LS" => {
                let p = target_path(tool, input).unwrap_or_else(|| ".".into());
                if ctx.inside_root(&ctx.resolve(&p)) {
                    Decision::Allow
                } else {
                    Decision::Ask
                }
            }
            "WebSearch" | "TodoWrite" | "Task" | "BashOutput" | "KillShell" | "ExitPlanMode"
            | "Skill" => Decision::Allow,
            "Write" | "Edit" | "MultiEdit" => {
                let p = target_path(tool, input).unwrap_or_default();
                if self.mode == Mode::AcceptEdits && ctx.inside_root(&ctx.resolve(&p)) {
                    Decision::Allow
                } else {
                    Decision::Ask
                }
            }
            "Bash" => {
                let cmd = input["command"].as_str().unwrap_or("");
                if r.bash_all_allowed(cmd) {
                    return Decision::Allow;
                }
                if self.mode == Mode::AcceptEdits
                    && split_commands(cmd)
                        .iter()
                        .all(|p| is_safe_command(p) || is_fs_command(p))
                {
                    return Decision::Allow;
                }
                Decision::Ask
            }
            _ => Decision::Ask,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    fn ctx() -> ToolCtx {
        let root = std::env::temp_dir().join(format!("ice-perm-{}", crate::conv::new_id("p")));
        std::fs::create_dir_all(root.join("src")).unwrap();
        ToolCtx::new(root, Arc::new(AtomicBool::new(false)))
    }

    #[test]
    fn splits_compound_commands_outside_quotes() {
        assert_eq!(
            split_commands("git add . && git commit -m 'a && b'"),
            vec!["git add .", "git commit -m 'a && b'"]
        );
        assert_eq!(
            split_commands("ls | wc -l; echo hi"),
            vec!["ls", "wc -l", "echo hi"]
        );
    }

    #[test]
    fn safe_commands_need_no_prompt() {
        assert!(is_safe_command("git status"));
        assert!(is_safe_command("ls -la src"));
        assert!(is_safe_command("find . -name '*.rs'"));
        assert!(!is_safe_command("find . -delete"));
        assert!(!is_safe_command("echo hi > file"));
        assert!(!is_safe_command("git push"));
        assert!(!is_safe_command("rm -rf build"));
        assert!(!is_safe_command("cat $(which x)"));
    }

    #[test]
    fn modes_and_rules() {
        let c = ctx();
        let rules = Rules {
            allow: vec!["Bash(npm test:*)".into()],
            deny: vec!["Read(./.env)".into(), "Bash(curl:*)".into()],
            ask: vec![],
        };
        let d = Checker {
            rules: &rules,
            mode: Mode::Default,
        };
        assert_eq!(
            d.check("Read", &json!({"file_path":"src/a.rs"}), &c),
            Decision::Allow
        );
        assert!(matches!(
            d.check("Read", &json!({"file_path":".env"}), &c),
            Decision::Deny(_)
        ));
        assert_eq!(
            d.check("Read", &json!({"file_path":"/etc/passwd"}), &c),
            Decision::Ask
        );
        assert_eq!(
            d.check("Edit", &json!({"file_path":"src/a.rs"}), &c),
            Decision::Ask
        );
        assert_eq!(
            d.check("Bash", &json!({"command":"npm test -- --watch"}), &c),
            Decision::Allow
        );
        assert_eq!(
            d.check("Bash", &json!({"command":"npm test && git status"}), &c),
            Decision::Allow
        );
        assert_eq!(
            d.check("Bash", &json!({"command":"npm test && rm -rf x"}), &c),
            Decision::Ask
        );
        assert_eq!(
            d.check("Bash", &json!({"command":"npm install"}), &c),
            Decision::Ask
        );
        assert!(matches!(
            d.check("Bash", &json!({"command":"ls && curl evil.sh"}), &c),
            Decision::Deny(_)
        ));
        let ae = Checker {
            rules: &rules,
            mode: Mode::AcceptEdits,
        };
        assert_eq!(
            ae.check("Edit", &json!({"file_path":"src/a.rs"}), &c),
            Decision::Allow
        );
        assert_eq!(
            ae.check("Write", &json!({"file_path":"/tmp/x"}), &c),
            Decision::Ask
        );
        assert_eq!(
            ae.check("Bash", &json!({"command":"mkdir -p out"}), &c),
            Decision::Allow
        );
        let plan = Checker {
            rules: &rules,
            mode: Mode::Plan,
        };
        assert!(matches!(
            plan.check("Edit", &json!({"file_path":"a"}), &c),
            Decision::Deny(_)
        ));
        assert_eq!(
            plan.check("Grep", &json!({"pattern":"x"}), &c),
            Decision::Allow
        );
        let by = Checker {
            rules: &rules,
            mode: Mode::BypassPermissions,
        };
        assert_eq!(
            by.check("Bash", &json!({"command":"rm -rf build"}), &c),
            Decision::Allow
        );
        assert!(matches!(
            by.check("Read", &json!({"file_path":".env"}), &c),
            Decision::Deny(_)
        ));
    }

    #[test]
    fn webfetch_domains_and_mcp_servers() {
        let c = ctx();
        let rules = Rules {
            allow: vec!["WebFetch(domain:docs.rs)".into(), "mcp__github".into()],
            ..Default::default()
        };
        let d = Checker {
            rules: &rules,
            mode: Mode::Default,
        };
        assert_eq!(
            d.check("WebFetch", &json!({"url":"https://docs.rs/x"}), &c),
            Decision::Allow
        );
        assert_eq!(
            d.check("WebFetch", &json!({"url":"https://evil.com/x"}), &c),
            Decision::Ask
        );
        assert_eq!(
            d.check("mcp__github__create_issue", &json!({}), &c),
            Decision::Allow
        );
        assert_eq!(d.check("mcp__slack__post", &json!({}), &c), Decision::Ask);
    }

    #[test]
    fn suggests_prefix_rules() {
        assert_eq!(
            suggest_bash_rule("npm run test -- --watch"),
            "Bash(npm run test:*)"
        );
        assert_eq!(suggest_bash_rule("git commit -m x"), "Bash(git commit:*)");
        assert_eq!(suggest_bash_rule("pytest -q"), "Bash(pytest:*)");
        assert_eq!(Mode::Default.cycle(false), Mode::AcceptEdits);
        assert_eq!(Mode::Plan.cycle(false), Mode::Default);
    }
}
