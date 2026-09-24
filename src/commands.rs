//! Slash commands, pickers and dialogs for the REPL.

use crate::editor::Editor;
use crate::engine::{PermReply, Stats};
use crate::providers::{self, IceConfig, PROVIDERS};
use crate::render::{self, Line, Style};
use crate::repl::{Cmd, Item, PermState, Picker, PickerKind, Repl};
use crate::theme::{Theme, ThemeName};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::{Path, PathBuf};

pub fn builtin_commands() -> Vec<(&'static str, &'static str)> {
    vec![
        ("agents", "List available sub-agents"),
        ("bashes", "List and manage background shells"),
        ("clear", "Clear conversation history and free up context"),
        ("compact", "Clear conversation history but keep a summary in context. Optional: /compact [instructions for summarization]"),
        ("config", "Open config panel"),
        ("context", "Visualize current context usage"),
        ("cost", "Show the total cost and duration of the current session"),
        ("doctor", "Diagnose and verify your ICE installation and settings"),
        ("exit", "Exit the REPL"),
        ("export", "Export the current conversation to a markdown file"),
        ("help", "Show help and available commands"),
        ("hooks", "Show configured hooks"),
        ("goal", "Give intent a direction: set a goal ICE keeps in every turn (/goal clear)"),
        ("init", "Initialize a new ICE.md file with codebase documentation"),
        ("login", "Choose a model provider and sign in (browser OAuth PKCE or API key)"),
        ("logout", "Remove saved API keys"),
        ("mcp", "Show MCP server connections and tools"),
        ("onboard", "Connect a provider: choose your model and sign in"),
        ("memory", "Edit ICE memory files"),
        ("model", "Set the AI model for ICE"),
        ("permissions", "Show and manage allow & deny tool permission rules"),
        ("providers", "List model providers and which have keys"),
        ("release-notes", "View release notes"),
        ("resume", "Resume a conversation"),
        ("review", "Review a pull request or your uncommitted changes"),
        ("rewind", "Rewind the conversation to a previous message"),
        ("skills", "List available skills"),
        ("status", "Show ICE status including version, model, account and settings"),
        ("terminal-setup", "Enable Shift+Enter for newlines"),
        ("theme", "Change the color theme"),
        ("todos", "List current todo items"),
        ("update", "Check for updates and install the latest version"),
    ]
}

/// Custom commands: `.ice/commands/*.md`, `.claude/commands/*.md`,
/// `~/.ice/commands/*.md`. Subdirectories become `dir:name`.
pub fn custom_commands(root: &Path) -> Vec<(String, String, PathBuf)> {
    let mut out: Vec<(String, String, PathBuf)> = Vec::new();
    let dirs = [
        (crate::settings::user_dir().join("commands"), "user"),
        (root.join(".claude/commands"), "project"),
        (root.join(".ice/commands"), "project"),
    ];
    for (dir, scope) in dirs {
        let walk = ignore::WalkBuilder::new(&dir)
            .max_depth(Some(3))
            .hidden(true)
            .build();
        for e in walk.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("md") {
                continue;
            }
            let rel = p.strip_prefix(&dir).unwrap_or(p).with_extension("");
            let name = rel.display().to_string().replace(['/', '\\'], ":");
            let body = std::fs::read_to_string(p).unwrap_or_default();
            let desc = frontmatter(&body, "description").unwrap_or_else(|| {
                strip_frontmatter(&body)
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
                    .chars()
                    .take(80)
                    .collect()
            });
            out.retain(|(n, _, _)| n != &name);
            out.push((name, format!("{desc} ({scope})"), p.to_path_buf()));
        }
    }
    out
}

fn frontmatter(body: &str, key: &str) -> Option<String> {
    let rest = body.strip_prefix("---")?;
    let end = rest.find("\n---")?;
    rest[..end].lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim() == key).then(|| v.trim().trim_matches('"').to_string())
    })
}

fn strip_frontmatter(body: &str) -> &str {
    if let Some(rest) = body.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            return rest[end + 4..].trim_start_matches(['\r', '\n']);
        }
    }
    body
}

pub fn all_commands(root: &Path) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = builtin_commands()
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    for (n, d, _) in custom_commands(root) {
        v.push((n, d));
    }
    v
}

fn fmt_duration(ms: u128) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}h {}m {}s", s / 3600, s % 3600 / 60, s % 60)
    } else if s >= 60 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{:.1}s", ms as f64 / 1000.0)
    }
}

pub fn cost_report(s: &Stats) -> String {
    let u = &s.usage;
    format!(
        "Total cost:            {}\nTotal duration (API):  {}\nTotal duration (wall): {}\nTotal code changes:    {} line{} added, {} line{} removed\nUsage:                 {} input, {} output, {} cache read, {} cache write",
        match s.cost {
            Some(c) => format!("${c:.4}"),
            None => "unknown for this model (token counts below)".into(),
        },
        fmt_duration(s.api_ms),
        fmt_duration(s.wall_ms),
        s.lines_added,
        if s.lines_added == 1 { "" } else { "s" },
        s.lines_removed,
        if s.lines_removed == 1 { "" } else { "s" },
        render::human_tokens(u.input_tokens),
        render::human_tokens(u.output_tokens),
        render::human_tokens(u.cache_read_input_tokens),
        render::human_tokens(u.cache_creation_input_tokens),
    )
}

pub fn context_report(
    parts: &[(String, u64)],
    used_reported: u64,
    window: u64,
    model: &str,
) -> String {
    let est: u64 = parts.iter().map(|(_, n)| n).sum();
    let used = used_reported.max(est);
    let pct = |n: u64| n as f64 / window.max(1) as f64 * 100.0;
    // A 10×10 grid of squares, like Claude Code's /context.
    let cells = 100usize;
    let mut grid = String::new();
    let mut filled = 0usize;
    let glyphs = ["⛁", "⛁", "⛀", "⛁", "⛁"];
    let mut marks: Vec<&str> = Vec::new();
    for (i, (_, n)) in parts.iter().enumerate() {
        let k = ((*n as f64 / window.max(1) as f64) * cells as f64).round() as usize;
        for _ in 0..k.min(cells - filled) {
            marks.push(glyphs[i % glyphs.len()]);
        }
        filled = marks.len();
    }
    while marks.len() < cells {
        marks.push("⛶");
    }
    for r in 0..10 {
        grid.push_str(&marks[r * 10..r * 10 + 10].join(" "));
        grid.push('\n');
    }
    let mut s = format!(
        "Context Usage\n{grid}\n{model} · {}/{} tokens ({:.0}%)\n",
        render::human_tokens(used),
        render::human_tokens(window),
        pct(used)
    );
    for (name, n) in parts {
        s.push_str(&format!(
            "  {name}: {} tokens ({:.1}%)\n",
            render::human_tokens(*n),
            pct(*n)
        ));
    }
    let free = window.saturating_sub(used);
    s.push_str(&format!(
        "  Free space: {} ({:.1}%)\n  Auto-compact buffer: {} (8%)",
        render::human_tokens(free),
        pct(free),
        render::human_tokens(window * 8 / 100)
    ));
    s
}

fn help_text(root: &Path) -> String {
    let mut s = format!(
        "ICE v{}\n\nAlways review ICE's responses, especially when running code. ICE has read access to files in the current directory and can run commands and edit files with your permission.\n\nUsage modes:\n• REPL: ice (interactive session)\n• Non-interactive: ice -p \"question\"\n\nRun ice -h for all command line flags\n\nCommon tasks:\n• Ask questions about your codebase > How does foo.py work?\n• Edit files > Update bar.ts to...\n• Fix errors > cargo build\n• Run commands > /help\n• Run bash commands > !ls\n\nInteractive mode commands:\n",
        env!("CARGO_PKG_VERSION")
    );
    for (n, d) in all_commands(root) {
        s.push_str(&format!("  /{n} - {d}\n"));
    }
    s.push_str("\nKeys: Enter send · \\+Enter or Alt+Enter newline · Esc interrupt · Esc Esc rewind · Shift+Tab cycle modes · Ctrl+R transcript · Ctrl+T todos · Ctrl+C exit");
    s
}

// ─────────────────────────── dispatch ───────────────────────────

pub fn slash(r: &mut Repl, line: &str) {
    let (name, arg) = match line[1..].split_once(char::is_whitespace) {
        Some((n, a)) => (n.to_string(), a.trim().to_string()),
        None => (line[1..].to_string(), String::new()),
    };
    // Custom commands expand into a prompt.
    if let Some((_, _, path)) = custom_commands(&r.root)
        .into_iter()
        .find(|(n, _, _)| *n == name)
    {
        let body = std::fs::read_to_string(&path).unwrap_or_default();
        let mut prompt = strip_frontmatter(&body).replace("$ARGUMENTS", &arg);
        for (i, a) in arg.split_whitespace().enumerate() {
            prompt = prompt.replace(&format!("${}", i + 1), a);
        }
        if r.running {
            r.queued.push(line.to_string());
            return;
        }
        r.start_turn(line.to_string(), prompt);
        return;
    }
    let echo = |r: &mut Repl| r.items.push(Item::User(line.to_string()));
    match name.as_str() {
        "exit" | "quit" => r.quit = true,
        "help" => {
            echo(r);
            r.items.push(Item::Info(help_text(&r.root)));
        }
        "clear" | "reset" | "new" => {
            if r.running {
                r.flash("Wait for the current turn to finish (or press Esc)");
                return;
            }
            r.send(Cmd::Clear);
        }
        "compact" => {
            if r.running {
                r.flash("Wait for the current turn to finish (or press Esc)");
                return;
            }
            echo(r);
            r.running = true;
            r.run_started = std::time::Instant::now();
            r.verb = "Compacting conversation";
            r.stream_chars = 0;
            r.send(Cmd::Compact(if arg.is_empty() { None } else { Some(arg) }));
        }
        "cost" => {
            echo(r);
            r.items.push(Item::Info(cost_report(&r.stats)));
        }
        "context" => {
            echo(r);
            r.send(Cmd::Context);
        }
        "status" => {
            echo(r);
            r.items.push(Item::Info(status_report(r)));
        }
        "doctor" => {
            echo(r);
            r.items
                .push(Item::Info(doctor_report(&r.root, r.mcp.as_deref())));
        }
        "init" => {
            let p = crate::prompt::INIT_PROMPT.to_string();
            r.start_turn(line.to_string(), p);
        }
        "review" => {
            let p = if arg.is_empty() {
                "You are an expert code reviewer. Review the uncommitted changes in this repository: run `git status`, `git diff` and `git diff --staged` (in parallel), read the surrounding code where needed, then give a concise review: an overview of what changed, correctness bugs and risks (with file:line references), and concrete suggestions. Order findings by severity.".to_string()
            } else {
                format!("You are an expert code reviewer. Review pull request {arg}: use `gh pr view {arg}` and `gh pr diff {arg}` to get the details, read the surrounding code where needed, then give a concise review: an overview, correctness bugs and risks (with file:line references), and concrete suggestions. Order findings by severity.")
            };
            r.start_turn(line.to_string(), p);
        }
        "model" => {
            if arg.is_empty() {
                open_model_picker(r);
            } else {
                echo(r);
                set_model(r, &arg);
            }
        }
        "login" | "onboard" => r.open_login(),
        "goal" => {
            echo(r);
            if arg.is_empty() {
                r.items.push(Item::Info(match &r.goal {
                    Some(g) => format!("Goal: {g}\n/goal clear to remove it"),
                    None => "No goal set. /goal <what you want to achieve> keeps ICE pointed at it for the whole session.".into(),
                }));
            } else if arg == "clear" {
                r.goal = None;
                r.send(Cmd::Goal(None));
                r.items.push(Item::Info("Goal cleared".into()));
            } else {
                r.goal = Some(arg.clone());
                r.send(Cmd::Goal(Some(arg.clone())));
                r.items.push(Item::Info(format!("Goal set: {arg}\nICE keeps this in mind in every turn. Send a message to start working toward it.")));
            }
        }
        "logout" => {
            echo(r);
            let n = crate::settings::remove_credentials();
            r.items.push(Item::Info(format!(
                "Removed {n} saved key{}. Keys set in your shell environment are unaffected.",
                if n == 1 { "" } else { "s" }
            )));
        }
        "resume" | "continue" => {
            if arg.is_empty() {
                r.open_resume();
            } else {
                r.send(Cmd::Resume(arg));
            }
        }
        "rewind" | "checkpoint" => open_rewind(r),
        "theme" => {
            if arg.is_empty() {
                open_theme_picker(r);
            } else if let Some(t) = ThemeName::parse(&arg) {
                set_theme(r, t);
            } else {
                r.flash(format!("Unknown theme {arg}"));
            }
        }
        "config" | "settings" => open_config(r),
        "mcp" => {
            echo(r);
            let text = match &r.mcp {
                Some(p) => crate::mcp::describe(p, &r.root),
                None => "MCP servers are still connecting…".into(),
            };
            r.items.push(Item::Info(text));
        }
        "agents" => {
            echo(r);
            r.items.push(Item::Info(crate::agents::catalog(&r.root)));
        }
        "skills" => {
            echo(r);
            r.items.push(Item::Info(crate::skills::catalog(&r.root)));
        }
        "hooks" => {
            echo(r);
            let h = crate::hooks::Hooks::load(&r.root);
            r.items.push(Item::Info(if h.is_empty() {
                "No hooks configured. Add them under \"hooks\" in .ice/settings.json (PreToolUse, PostToolUse, UserPromptSubmit, Stop, SessionStart).".into()
            } else {
                format!("{} hook command{} configured (see .ice/settings.json and ~/.ice/settings.json).", h.count(), if h.count() == 1 { "" } else { "s" })
            }));
        }
        "todos" => {
            echo(r);
            let text = if r.todos.is_empty() {
                "No todos currently tracked".to_string()
            } else {
                r.todos
                    .iter()
                    .map(|t| {
                        format!(
                            "{} {}",
                            if t.status == "completed" {
                                "☒"
                            } else {
                                "☐"
                            },
                            t.content
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            r.items.push(Item::Info(text));
        }
        "bashes" => open_bashes(r),
        "memory" => {
            if arg.is_empty() {
                open_memory_files(r);
            } else {
                open_memory_picker(r, arg);
            }
        }
        "permissions" | "allowed-tools" => {
            echo(r);
            let out = permissions_cmd(r, &arg);
            r.items.push(Item::Info(out));
        }
        "providers" | "provider" => {
            echo(r);
            r.items.push(Item::Info(providers_report()));
        }
        "export" => {
            echo(r);
            let path = if arg.is_empty() {
                r.root
                    .join(format!("ice-conversation-{}.md", crate::prompt::today()))
            } else {
                r.root.join(&arg)
            };
            let md = export_markdown(&r.items);
            r.items.push(Item::Info(match std::fs::write(&path, md) {
                Ok(()) => format!("Conversation exported to: {}", path.display()),
                Err(e) => format!("Failed to export: {e}"),
            }));
        }
        "release-notes" => {
            echo(r);
            r.items.push(Item::Info(release_notes()));
        }
        "update" | "upgrade" => {
            echo(r);
            r.items.push(Item::Info("Run `ice update` in your shell to update. ICE checks for new versions automatically at startup.".into()));
        }
        "terminal-setup" => {
            echo(r);
            r.items.push(Item::Info("Newlines: type \\ then Enter, or Alt+Enter / Option+Enter, or Ctrl+J — these work in every terminal.\nShift+Enter works in terminals with the kitty keyboard protocol (kitty, WezTerm, Ghostty, foot, recent iTerm2 with \"Report keys using CSI u\").\nVS Code terminal: add a keybinding for shift+enter that sends \"\\u001b\\r\".".into()));
        }
        other => {
            r.flash(format!("Unknown command: /{other} · /help for commands"));
        }
    }
}

fn set_model(r: &mut Repl, m: &str) {
    let m = crate::model::resolve_alias(&IceConfig::load().provider, m);
    let mut cfg = IceConfig::load();
    cfg.model = m.clone();
    let _ = cfg.save();
    std::env::remove_var("ICE_MODEL");
    r.send(Cmd::Model(m.clone()));
    r.items.push(Item::Info(format!("Set model to {m}")));
}

fn set_theme(r: &mut Repl, t: ThemeName) {
    r.theme_name = t;
    r.theme = t.palette();
    let _ = crate::settings::set_user_value("theme", serde_json::json!(t.id()));
    r.redraw_all();
    r.flash(format!("Theme set to {}", t.id()));
}

fn status_report(r: &Repl) -> String {
    let s = &r.stats;
    let cfg = IceConfig::load();
    let key_env = providers::find(&cfg.provider)
        .map(|p| p.key_envs[0])
        .unwrap_or("ICE_API_KEY");
    let key = std::env::var(key_env).ok().filter(|k| !k.is_empty());
    let memory = crate::prompt::load_memory(&r.root);
    let mut out = format!("ICE v{}\n\n", env!("CARGO_PKG_VERSION"));
    out.push_str(&format!(
        "Session ID: {}\ncwd: {}\n\n",
        s.session_id,
        r.root.display()
    ));
    out.push_str(&format!(
        "Provider: {}\nModel: {}\nBase URL: {}\nAPI key: {}\nTool calling: {}\n\n",
        if cfg.provider.is_empty() {
            "(none)".into()
        } else {
            cfg.provider.clone()
        },
        s.model,
        if s.base_url.is_empty() {
            "-".into()
        } else {
            s.base_url.clone()
        },
        match &key {
            Some(k) => format!(
                "{} ({}, {})",
                crate::settings::mask(k),
                key_env,
                crate::settings::credential_source(key_env).unwrap_or("environment")
            ),
            None => format!("missing — set {key_env} or run /login"),
        },
        if s.text_tools {
            "text protocol (model lacks native tools)"
        } else {
            "native"
        },
    ));
    out.push_str(&format!("Permission mode: {}\n", r.mode.id()));
    out.push_str(&format!(
        "Memory: {}\n",
        if memory.is_empty() {
            "none (run /init)".into()
        } else {
            memory
                .iter()
                .map(|m| m.path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        }
    ));
    let mcp = r
        .mcp
        .as_ref()
        .map(|m| {
            format!(
                "{} connected, {} tools",
                m.status.iter().filter(|(_, x)| x.is_ok()).count(),
                m.tools.len()
            )
        })
        .unwrap_or_else(|| "connecting…".into());
    out.push_str(&format!("MCP servers: {mcp}\n"));
    let settings: Vec<String> = [
        crate::settings::user_path(),
        crate::settings::project_path(&r.root),
        crate::settings::local_path(&r.root),
    ]
    .iter()
    .filter(|p| p.exists())
    .map(|p| p.display().to_string())
    .collect();
    out.push_str(&format!(
        "Settings: {}",
        if settings.is_empty() {
            "defaults".into()
        } else {
            settings.join(", ")
        }
    ));
    out
}

pub fn doctor_report(root: &Path, mcp: Option<&crate::mcp::Pool>) -> String {
    let ok = |b: bool| if b { "✔" } else { "✘" };
    let mut s = String::from("ICE Diagnostics\n");
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "?".into());
    s.push_str(&format!(
        "{} Version {} ({} {})\n",
        ok(true),
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    s.push_str(&format!("{} Executable: {exe}\n", ok(true)));
    s.push_str(&format!(
        "{} Install method: {}\n",
        ok(true),
        crate::update::install_method()
    ));
    let bash = crate::tools::which("bash") || std::path::Path::new("/bin/bash").exists();
    s.push_str(&format!(
        "{} bash available{}\n",
        ok(bash),
        if bash {
            ""
        } else {
            " — install Git for Windows (bash.exe) for the Bash tool"
        }
    ));
    let git = crate::tools::which("git") || crate::tools::which("git.exe");
    s.push_str(&format!("{} git available\n", ok(git)));
    let dir = crate::settings::user_dir();
    let writable =
        std::fs::create_dir_all(&dir).is_ok() && std::fs::write(dir.join(".doctor"), "ok").is_ok();
    let _ = std::fs::remove_file(dir.join(".doctor"));
    s.push_str(&format!(
        "{} Config dir writable: {}\n",
        ok(writable),
        dir.display()
    ));
    let cfg = crate::model::ModelConfig::resolve(None);
    s.push_str(&format!(
        "{} Model: {}\n",
        ok(cfg.is_some()),
        cfg.as_ref()
            .map(|c| format!("{} via {} ({})", c.model, c.provider_id, c.base_url))
            .unwrap_or_else(|| "not configured — run /login".into())
    ));
    for p in [
        crate::settings::user_path(),
        crate::settings::project_path(root),
        crate::settings::local_path(root),
    ] {
        if let Ok(t) = std::fs::read_to_string(&p) {
            let valid = serde_json::from_str::<serde_json::Value>(&t).is_ok();
            s.push_str(&format!(
                "{} Settings file {}{}\n",
                ok(valid),
                p.display(),
                if valid { "" } else { " is not valid JSON" }
            ));
        }
    }
    for (p, _) in crate::mcp::config_paths(root) {
        if let Ok(t) = std::fs::read_to_string(&p) {
            let valid = serde_json::from_str::<serde_json::Value>(&t).is_ok();
            s.push_str(&format!(
                "{} MCP config {}{}\n",
                ok(valid),
                p.display(),
                if valid { "" } else { " is not valid JSON" }
            ));
        }
    }
    if let Some(m) = mcp {
        for (n, st) in &m.status {
            match st {
                Ok(k) => s.push_str(&format!("✔ MCP {n}: {k} tools\n")),
                Err(e) => s.push_str(&format!("✘ MCP {n}: {e}\n")),
            }
        }
    }
    let (w, h) = crossterm::terminal::size().unwrap_or((0, 0));
    s.push_str(&format!(
        "{} Terminal {w}×{h}, {}",
        ok(w >= 60),
        if crate::theme::truecolor() {
            "truecolor"
        } else {
            "256 colours"
        }
    ));
    s
}

fn providers_report() -> String {
    let cfg = IceConfig::load();
    let mut s = String::from("Providers (● key available):\n");
    for p in PROVIDERS {
        let has = providers::key_for(p).is_some() || p.id == "ollama";
        s.push_str(&format!(
            "  {} {:<11} {:<40} {}{}\n",
            if has { "●" } else { "○" },
            p.id,
            p.default_model,
            p.key_envs[0],
            if p.id == cfg.provider {
                "  ← active"
            } else {
                ""
            }
        ));
    }
    s.push_str("\nCLI backends (use your existing login in another agent CLI):\n");
    for c in providers::CLI_AGENTS {
        s.push_str(&format!(
            "  {} cli:{:<9} {}\n",
            if providers::cli_agent_present(c) {
                "●"
            } else {
                "○"
            },
            c.id,
            c.name
        ));
    }
    s.push_str("\nSwitch with /login.");
    s
}

fn permissions_cmd(r: &mut Repl, arg: &str) -> String {
    let mut words = arg.splitn(2, ' ');
    let verb = words.next().unwrap_or("");
    let rule = words.next().unwrap_or("").trim();
    if matches!(verb, "allow" | "deny" | "ask") && !rule.is_empty() {
        let p = crate::settings::local_path(&r.root);
        let mut v: serde_json::Value = std::fs::read_to_string(&p)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        if !v["permissions"].is_object() {
            v["permissions"] = serde_json::json!({});
        }
        let mut list: Vec<String> = v["permissions"][verb]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if !list.iter().any(|x| x == rule) {
            list.push(rule.to_string());
        }
        v["permissions"][verb] = serde_json::json!(list);
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(
            &p,
            serde_json::to_string_pretty(&v).unwrap_or_default() + "\n",
        );
        r.send(Cmd::ReloadRules);
        return format!("Added {verb} rule {rule} to {}", p.display());
    }
    if verb == "remove" && !rule.is_empty() {
        let mut removed = false;
        for p in [
            crate::settings::local_path(&r.root),
            crate::settings::project_path(&r.root),
            crate::settings::user_path(),
        ] {
            let Ok(t) = std::fs::read_to_string(&p) else {
                continue;
            };
            let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&t) else {
                continue;
            };
            for k in ["allow", "deny", "ask"] {
                if let Some(a) = v["permissions"][k].as_array_mut() {
                    let before = a.len();
                    a.retain(|x| x.as_str() != Some(rule));
                    if a.len() != before {
                        removed = true;
                    }
                }
            }
            let _ = std::fs::write(
                &p,
                serde_json::to_string_pretty(&v).unwrap_or_default() + "\n",
            );
        }
        r.send(Cmd::ReloadRules);
        return if removed {
            format!("Removed rule {rule}")
        } else {
            format!("No rule {rule} found")
        };
    }
    let s = crate::settings::Settings::load(&r.root);
    let list = |v: &[String]| {
        if v.is_empty() {
            "  (none)".to_string()
        } else {
            v.iter()
                .map(|x| format!("  {x}"))
                .collect::<Vec<_>>()
                .join("\n")
        }
    };
    format!(
        "Permission mode: {}\n\nAllow:\n{}\n\nAsk:\n{}\n\nDeny:\n{}\n\nManage with /permissions allow <rule>, /permissions deny <rule>, /permissions remove <rule>.\nRules look like Bash(npm test:*), Edit(src/**), Read(~/.ssh/**), WebFetch(domain:docs.rs), mcp__server.",
        r.mode.id(),
        list(&s.allow),
        list(&s.ask),
        list(&s.deny)
    )
}

fn export_markdown(items: &[Item]) -> String {
    let mut s = format!("# ICE conversation — {}\n\n", crate::prompt::today());
    for it in items {
        match it {
            Item::User(t) => s.push_str(&format!("## > {}\n\n", t.replace('\n', "\n> "))),
            Item::Assistant(t) => s.push_str(&format!("{t}\n\n")),
            Item::Tool {
                name,
                input,
                output,
                ..
            } => {
                s.push_str(&format!(
                    "**Tool** `{name}` `{}`\n\n",
                    render::compact_json(input, 200)
                ));
                if let Some(o) = output {
                    let c: String = o.content.chars().take(2000).collect();
                    s.push_str(&format!("```\n{c}\n```\n\n"));
                }
            }
            Item::Bash { command, output } => {
                s.push_str(&format!("```\n$ {command}\n{}\n```\n\n", output.content))
            }
            Item::Info(t) | Item::Notice(t) => {
                s.push_str(&format!("_{}_\n\n", t.lines().next().unwrap_or("")))
            }
            Item::Error(e) => s.push_str(&format!("**Error:** {e}\n\n")),
            _ => {}
        }
    }
    s
}

fn release_notes() -> String {
    format!(
        "ICE {} — highlights:\n• Native tool calling for Anthropic and OpenAI-compatible APIs, streaming responses, retries with backoff\n• Claude-Code-compatible tools: Read, Write, Edit, MultiEdit, Glob, Grep, LS, Bash (+ background shells), WebFetch, WebSearch, TodoWrite, Task sub-agents\n• Permission modes (default / acceptEdits / plan / bypass) with allow/deny rules and hooks\n• Sessions saved to ~/.ice/projects — ice --continue / --resume\n• ICE.md project memory, custom slash commands, custom agents, skills, MCP (stdio + HTTP)\n• Print mode with text / json / stream-json output",
        env!("CARGO_PKG_VERSION")
    )
}

// ─────────────────────────── permission dialog ───────────────────────────

pub fn perm_options(p: &PermState) -> Vec<(String, PermReply)> {
    let req = &p.req;
    if req.tool == "ExitPlanMode" {
        return vec![
            ("Yes, and auto-accept edits".into(), PermReply::AllowAlways),
            ("Yes, and manually approve edits".into(), PermReply::Allow),
            ("No, keep planning".into(), PermReply::Deny(None)),
        ];
    }
    let mut v = vec![("Yes".to_string(), PermReply::Allow)];
    if let Some(a) = &req.always_label {
        v.push((a.clone(), PermReply::AllowAlways));
    }
    v.push((
        "No, and tell ICE what to do differently (esc)".into(),
        PermReply::Deny(None),
    ));
    v
}

fn boxed(title: &str, title_style: Style, body: Vec<Line>, border: Style, w: usize) -> Vec<Line> {
    let inner = w.saturating_sub(4);
    let mut out = vec![Line::styled(format!("╭{}╮", "─".repeat(w - 2)), border)];
    let mut rows = vec![Line::styled(title, title_style)];
    rows.extend(body);
    for l in rows {
        // URLs wrap flush-left so they stay copyable.
        let indent = if l.text().trim_start().starts_with("http") {
            0
        } else {
            2
        };
        for wl in render::wrap(&l, inner, indent) {
            let pad = inner.saturating_sub(wl.width());
            let mut row = Line::styled("│ ", border);
            let fill = wl.fill;
            row.spans.extend(wl.spans);
            if let Some(bg) = fill {
                row.add(" ".repeat(pad), Style::default().on(bg));
            } else {
                row.add(" ".repeat(pad), Style::default());
            }
            row.add(" │", border);
            out.push(row);
        }
    }
    out.push(Line::styled(format!("╰{}╯", "─".repeat(w - 2)), border));
    out
}

pub fn permission_box(p: &PermState, w: usize, t: &Theme, _root: &Path) -> Vec<Line> {
    let (title, mut body, question) =
        render::permission_body(&p.req.tool, &p.req.detail, w.saturating_sub(4), t);
    body.insert(0, Line::new());
    body.push(Line::new());
    body.push(Line::raw(question));
    for (i, (label, _)) in perm_options(p).iter().enumerate() {
        let sel = i == p.idx;
        let st = if sel {
            Style::fg(t.accent)
        } else {
            Style::default()
        };
        body.push(
            Line::styled(if sel { "❯ " } else { "  " }, st).push(format!("{}. {label}", i + 1), st),
        );
    }
    let border = Style::fg(if p.req.tool == "ExitPlanMode" {
        t.plan
    } else {
        t.warn
    });
    boxed(
        &title,
        Style::fg(if p.req.tool == "ExitPlanMode" {
            t.plan
        } else {
            t.warn
        })
        .bold(),
        body,
        border,
        w,
    )
}

pub fn picker_box(p: &Picker, w: usize, t: &Theme) -> (Vec<Line>, Option<(usize, usize)>) {
    let mut body = vec![Line::new()];
    body.extend(p.body.iter().cloned());
    if !p.body.is_empty() {
        body.push(Line::new());
    }
    let mut cursor = None;
    if let Some(e) = &p.entry {
        let inner = w.saturating_sub(8);
        let (rows, (cr, cc)) = e.layout(inner, p.masked);
        let first = rows.first().cloned().unwrap_or_default();
        let row_idx = body.len();
        body.push(Line::styled("> ", Style::fg(t.accent)).push(
            if e.is_empty() && p.masked {
                String::new()
            } else {
                first
            },
            Style::default(),
        ));
        for r in rows.iter().skip(1) {
            body.push(Line::raw(format!("  {r}")));
        }
        // +1 for the top border, +1 for the title row.
        cursor = Some((row_idx + 2 + cr, 2 + 2 + cc));
        body.push(Line::new());
    }
    // Keep long lists windowed around the selection.
    let max = 12usize;
    let start = if p.options.len() > max {
        p.idx.saturating_sub(max / 2).min(p.options.len() - max)
    } else {
        0
    };
    for (i, (label, hint)) in p.options.iter().enumerate().skip(start).take(max) {
        let sel = i == p.idx && p.entry.is_none();
        let st = if sel {
            Style::fg(t.accent)
        } else {
            Style::default()
        };
        let mut l =
            Line::styled(if sel { "❯ " } else { "  " }, st).push(format!("{}. {label}", i + 1), st);
        if !hint.is_empty() {
            l.add(format!("  {hint}"), Style::fg(t.dim));
        }
        body.push(l);
    }
    if p.options.len() > max {
        body.push(Line::styled(
            format!("  ({} of {})", p.idx + 1, p.options.len()),
            Style::fg(t.dim),
        ));
    }
    body.push(Line::new());
    body.push(Line::styled(
        if p.options.is_empty() && p.entry.is_none() {
            "Esc to cancel"
        } else if p.entry.is_some() {
            "Enter to confirm · Esc to cancel"
        } else {
            "↑/↓ to select · Enter to confirm · Esc to cancel"
        },
        Style::fg(t.dim),
    ));
    let lines = boxed(
        &p.title,
        Style::fg(t.accent).bold(),
        body,
        Style::fg(t.border),
        w,
    );
    (lines, cursor)
}

pub fn close_picker(r: &mut Repl) {
    if let Some(p) = r.picker.take() {
        match p.kind {
            PickerKind::Trust => r.quit = true,
            PickerKind::OAuthWait => r
                .oauth_cancel
                .store(true, std::sync::atomic::Ordering::Relaxed),
            _ => {}
        }
    }
}

fn picker(
    title: &str,
    body: Vec<Line>,
    options: Vec<(String, String)>,
    kind: PickerKind,
) -> Picker {
    Picker {
        title: title.into(),
        body,
        options,
        idx: 0,
        kind,
        entry: None,
        masked: false,
    }
}

pub fn on_picker_key(r: &mut Repl, k: KeyEvent) {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let Some(p) = &mut r.picker else { return };
    if let Some(e) = &mut p.entry {
        match k.code {
            KeyCode::Esc => return close_picker(r),
            KeyCode::Enter => {}
            KeyCode::Backspace => {
                e.backspace();
                return;
            }
            KeyCode::Left => {
                e.left();
                return;
            }
            KeyCode::Right => {
                e.right();
                return;
            }
            KeyCode::Char('u') if ctrl => {
                e.kill_to_start();
                return;
            }
            KeyCode::Char(c) if !ctrl => {
                e.insert(c);
                return;
            }
            _ => return,
        }
    } else {
        let n = p.options.len().max(1);
        match k.code {
            KeyCode::Esc => return close_picker(r),
            KeyCode::Up | KeyCode::Char('k') => {
                p.idx = (p.idx + n - 1) % n;
                return;
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                p.idx = (p.idx + 1) % n;
                return;
            }
            KeyCode::Char(c @ '1'..='9') => {
                let i = c as usize - '1' as usize;
                if i < p.options.len() {
                    p.idx = i;
                } else {
                    return;
                }
            }
            KeyCode::Enter => {}
            _ => return,
        }
    }
    let p = r.picker.take().unwrap();
    let choice = p.idx;
    let text = p
        .entry
        .as_ref()
        .map(|e| e.text.trim().to_string())
        .unwrap_or_default();
    match p.kind {
        PickerKind::Trust => {
            if choice == 0 {
                let _ = IceConfig::set_trusted(&r.root);
                if crate::model::ModelConfig::resolve(None).is_none()
                    && providers::selected_cli_agent().is_none()
                {
                    r.open_login();
                }
            } else {
                r.quit = true;
            }
        }
        PickerKind::Theme => {
            if let Some(t) = ThemeName::all().get(choice) {
                set_theme(r, *t);
            }
        }
        PickerKind::Model(list) => {
            if let Some(m) = list.get(choice) {
                set_model(r, m);
            }
        }
        PickerKind::Resume(list) => {
            if let Some(s) = list.get(choice) {
                r.send(Cmd::Resume(s.id.clone()));
            }
        }
        PickerKind::Rewind(list) => {
            if let Some((k, text)) = list.get(choice).cloned() {
                r.send(Cmd::Rewind(k));
                // Drop the rewound items from the display and put the
                // message back in the prompt for editing.
                let mut seen = 0;
                let mut cut = r.items.len();
                for (i, it) in r.items.iter().enumerate() {
                    if matches!(it, Item::User(_)) {
                        if seen == k {
                            cut = i;
                            break;
                        }
                        seen += 1;
                    }
                }
                r.items.truncate(cut);
                r.redraw_all();
                r.input.set(&text);
            }
        }
        PickerKind::Memory(note) => {
            let path = match choice {
                0 => r.root.join("ICE.md"),
                1 => r.root.join("ICE.local.md"),
                _ => crate::settings::user_dir().join("ICE.md"),
            };
            let mut cur = std::fs::read_to_string(&path).unwrap_or_default();
            if !cur.is_empty() && !cur.ends_with('\n') {
                cur.push('\n');
            }
            cur.push_str(&format!("- {note}\n"));
            if let Some(d) = path.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            r.items.push(Item::User(format!("# {note}")));
            r.items.push(Item::Info(match std::fs::write(&path, cur) {
                Ok(()) => format!("Got it. Saved to {}", path.display()),
                Err(e) => format!("Could not save memory: {e}"),
            }));
        }
        PickerKind::OpenMemory => {
            let path = match choice {
                0 => r.root.join("ICE.md"),
                1 => r.root.join("ICE.local.md"),
                _ => crate::settings::user_dir().join("ICE.md"),
            };
            open_in_editor(r, &path);
        }
        PickerKind::Config => {
            config_toggle(r, choice);
            open_config(r);
            if let Some(p) = &mut r.picker {
                p.idx = choice;
            }
        }
        PickerKind::Bashes(ids) => {
            if let (Some(id), Some(ctx)) = (ids.get(choice), &r.ctx) {
                let out = crate::tools::run(ctx, "KillShell", &serde_json::json!({"shell_id": id}));
                r.flash(out.content);
            }
        }
        PickerKind::Login => login_choose(r, choice),
        PickerKind::LoginUrl { provider } => {
            if !text.is_empty() {
                let mut cfg = IceConfig::load();
                cfg.base_url = Some(text);
                cfg.provider = PROVIDERS[provider].id.into();
                let _ = cfg.save();
            }
            open_login_method(r, provider);
        }
        PickerKind::LoginKey { provider } => {
            let p = &PROVIDERS[provider];
            if !text.is_empty() {
                if let Err(e) = crate::settings::save_credential(p.key_envs[0], &text) {
                    r.flash(format!("Could not save key: {e}"));
                }
            }
            let mut cfg = IceConfig::load();
            cfg.provider = p.id.into();
            if cfg
                .base_url
                .as_deref()
                .map(|b| PROVIDERS.iter().any(|x| x.base_url == b && x.id != p.id))
                .unwrap_or(false)
                || p.id != "custom"
            {
                cfg.base_url = if p.id == "custom" { cfg.base_url } else { None };
            }
            cfg.model = p.default_model.into();
            cfg.onboarded = true;
            let _ = cfg.save();
            std::env::remove_var("ICE_PROVIDER");
            std::env::remove_var("ICE_MODEL");
            r.send(Cmd::ReloadModel);
            if providers::key_for(p).is_none() && p.id != "ollama" && p.id != "custom" {
                r.items.push(Item::Error(format!(
                    "No key entered for {}. Set {} or run /login again.",
                    p.name, p.key_envs[0]
                )));
            } else {
                r.items.push(Item::Info(format!(
                    "Login successful · {} · model {} (change with /model)",
                    p.name, p.default_model
                )));
            }
            if p.id == "custom" || p.id == "ollama" {
                open_model_picker(r);
            }
        }
        PickerKind::LoginMethod { provider } => {
            if choice == 0 {
                start_oauth(r, provider);
            } else {
                open_login_key(r, provider);
            }
        }
        PickerKind::OAuthWait => {}
    }
}

/// Auth-method selector shown after choosing a provider at onboarding.
fn open_login_method(r: &mut Repl, provider: usize) {
    let p = &PROVIDERS[provider];
    let (oauth_hint, ready) = match crate::oauth::availability(p.id) {
        crate::oauth::Availability::Ready(_) => (
            if crate::oauth::signed_in(p.id) {
                "signed in · sign in again".to_string()
            } else {
                "opens your browser · no key to copy".to_string()
            },
            true,
        ),
        crate::oauth::Availability::NeedsSetup(_) => (
            "needs your OAuth app (client id) — shows how".to_string(),
            false,
        ),
    };
    let key_hint = match providers::key_for(p) {
        Some(k) => format!("{} found", crate::settings::mask(&k)),
        None => p.key_envs[0].to_string(),
    };
    let mut pk = picker(
        &format!("Sign in to {}", p.name),
        vec![Line::raw("How do you want to authenticate?")],
        vec![
            ("Sign in with browser (OAuth 2.0 + PKCE)".into(), oauth_hint),
            ("Paste an API key".into(), key_hint),
        ],
        PickerKind::LoginMethod { provider },
    );
    pk.idx = if ready { 0 } else { 1 };
    r.picker = Some(pk);
}

fn start_oauth(r: &mut Repl, provider: usize) {
    let p = &PROVIDERS[provider];
    let cfg = match crate::oauth::availability(p.id) {
        crate::oauth::Availability::Ready(c) => c,
        crate::oauth::Availability::NeedsSetup(msg) => {
            r.items.push(Item::Info(msg));
            open_login_key(r, provider);
            return;
        }
    };
    let pending = match crate::oauth::begin(cfg) {
        Ok(p) => p,
        Err(e) => {
            r.items
                .push(Item::Error(format!("Could not start browser sign-in: {e}")));
            return;
        }
    };
    let url = pending.url.clone();
    let opened = crate::oauth::open_browser(&url);
    let t = r.theme;
    let body = vec![
        Line::raw(if opened {
            "Your browser has been opened to sign in."
        } else {
            "Open this link in your browser to sign in:"
        }),
        Line::new(),
        Line::styled(url, Style::fg(t.accent).underlined()),
        Line::new(),
        Line::styled(
            "Waiting for the browser to redirect back to ICE… (PKCE S256, local callback)",
            Style::fg(t.dim),
        ),
    ];
    r.picker = Some(picker(
        &format!("Signing in to {}", p.name),
        body,
        vec![],
        PickerKind::OAuthWait,
    ));
    r.oauth_cancel
        .store(false, std::sync::atomic::Ordering::Relaxed);
    let cancel = r.oauth_cancel.clone();
    let tx = r.bg_tx.clone();
    std::thread::spawn(move || {
        let res = crate::oauth::finish(pending, &cancel).map_err(|e| e.to_string());
        let _ = tx.send(crate::repl::Bg::OAuth(provider, res));
    });
}

pub fn oauth_finished(r: &mut Repl, provider: usize, res: Result<crate::oauth::Tokens, String>) {
    let p = &PROVIDERS[provider];
    match res.and_then(|t| crate::oauth::store(p.id, &t).map_err(|e| e.to_string())) {
        Ok(()) => {
            let mut cfg = IceConfig::load();
            cfg.provider = p.id.into();
            cfg.base_url = None;
            cfg.model = p.default_model.into();
            cfg.onboarded = true;
            let _ = cfg.save();
            std::env::remove_var("ICE_PROVIDER");
            std::env::remove_var("ICE_MODEL");
            r.send(Cmd::ReloadModel);
            r.items.push(Item::Info(format!(
                "Signed in to {} with your browser (OAuth PKCE) · model {} (change with /model)",
                p.name, p.default_model
            )));
        }
        Err(e) if e.contains("cancelled") => r.items.push(Item::Notice("Sign-in cancelled".into())),
        Err(e) => r
            .items
            .push(Item::Error(format!("Browser sign-in failed: {e}"))),
    }
}

fn login_choose(r: &mut Repl, choice: usize) {
    if choice < PROVIDERS.len() {
        let p = &PROVIDERS[choice];
        if p.id == "custom" {
            let mut pk = picker(
                "Custom OpenAI-compatible endpoint",
                vec![Line::raw("Base URL (e.g. http://localhost:8000/v1):")],
                vec![],
                PickerKind::LoginUrl { provider: choice },
            );
            let mut e = Editor::default();
            e.set(&IceConfig::load().base_url.unwrap_or_default());
            pk.entry = Some(e);
            r.picker = Some(pk);
            return;
        }
        if p.id == "ollama" {
            let mut cfg = IceConfig::load();
            cfg.provider = "ollama".into();
            cfg.base_url = None;
            cfg.model = p.default_model.into();
            cfg.onboarded = true;
            let _ = cfg.save();
            std::env::remove_var("ICE_PROVIDER");
            r.send(Cmd::ReloadModel);
            r.items.push(Item::Info(
                "Using local Ollama (http://127.0.0.1:11434). Pick a model:".into(),
            ));
            open_model_picker(r);
            return;
        }
        open_login_method(r, choice);
    } else {
        let c =
            &providers::CLI_AGENTS[(choice - PROVIDERS.len()).min(providers::CLI_AGENTS.len() - 1)];
        let mut cfg = IceConfig::load();
        cfg.provider = format!("cli:{}", c.id);
        cfg.model = c.id.into();
        cfg.base_url = None;
        cfg.onboarded = true;
        let _ = cfg.save();
        std::env::remove_var("ICE_PROVIDER");
        r.send(Cmd::ReloadModel);
        let present = providers::cli_agent_present(c);
        r.items.push(Item::Info(format!(
            "Backend: {}{}\n{}",
            c.name,
            if present {
                ""
            } else {
                " (not found on PATH — install it first)"
            },
            c.login_hint
        )));
    }
}

fn open_login_key(r: &mut Repl, provider: usize) {
    let p = &PROVIDERS[provider];
    let existing = providers::key_for(p);
    let mut body = vec![Line::raw(format!(
        "Paste your {} API key ({}).",
        p.name, p.key_envs[0]
    ))];
    if let Some(k) = &existing {
        body.push(Line::raw(format!(
            "A key is already available ({}). Press Enter to keep it.",
            crate::settings::mask(k)
        )));
    }
    body.push(Line::raw(format!(
        "It is stored in {} (readable only by you).",
        crate::settings::credentials_path().display()
    )));
    let mut pk = picker(
        &format!("Log in to {}", p.name),
        body,
        vec![],
        PickerKind::LoginKey { provider },
    );
    pk.entry = Some(Editor::default());
    pk.masked = true;
    r.picker = Some(pk);
}

impl Repl {
    pub fn open_trust(&mut self) {
        let t = self.theme;
        let body = vec![
            Line::styled(self.root.display().to_string(), Style::default().bold()),
            Line::new(),
            Line::raw("ICE may read files in this folder. Reading untrusted files may lead ICE to behave in unexpected ways."),
            Line::new(),
            Line::raw("With your permission ICE may execute files in this folder. Executing untrusted code is unsafe."),
            Line::styled("Security guide: review what ICE proposes before approving it.", Style::fg(t.dim)),
        ];
        self.picker = Some(picker(
            "Do you trust the files in this folder?",
            body,
            vec![
                ("Yes, proceed".into(), String::new()),
                ("No, exit".into(), String::new()),
            ],
            PickerKind::Trust,
        ));
    }

    pub fn open_login(&mut self) {
        let mut options: Vec<(String, String)> = PROVIDERS
            .iter()
            .map(|p| {
                let has = providers::key_for(p).is_some();
                (
                    p.name.to_string(),
                    if has {
                        format!("{} ✔ key found", p.default_model)
                    } else {
                        p.default_model.to_string()
                    },
                )
            })
            .collect();
        for c in providers::CLI_AGENTS {
            options.push((
                c.name.to_string(),
                if providers::cli_agent_present(c) {
                    "installed · uses its own login".into()
                } else {
                    "not installed".into()
                },
            ));
        }
        let cur = IceConfig::load().provider;
        let idx = PROVIDERS.iter().position(|p| p.id == cur).unwrap_or(0);
        let mut pk = picker(
            "Select a model provider",
            vec![Line::raw(
                "ICE works with hosted APIs, local models and your existing coding-agent CLIs.",
            )],
            options,
            PickerKind::Login,
        );
        pk.idx = idx;
        self.picker = Some(pk);
    }

    pub fn open_resume(&mut self) {
        let list = crate::store::list(&self.root);
        if list.is_empty() {
            self.flash("No conversations found to resume");
            return;
        }
        let options = list
            .iter()
            .take(50)
            .map(|s| {
                (
                    render::truncate(&s.title, 60),
                    format!(
                        "{} · {} messages",
                        crate::store::ago(s.modified),
                        s.messages
                    ),
                )
            })
            .collect();
        self.picker = Some(picker(
            "Resume a conversation",
            vec![],
            options,
            PickerKind::Resume(list.into_iter().take(50).collect()),
        ));
    }
}

fn open_model_picker(r: &mut Repl) {
    let cfg = IceConfig::load();
    let mut models: Vec<String> = Vec::new();
    let mut options: Vec<(String, String)> = Vec::new();
    if cfg.provider == "anthropic" {
        for (alias, desc) in [
            ("sonnet", "Sonnet — best for everyday coding"),
            ("opus", "Opus — most capable for complex work"),
            ("haiku", "Haiku — fastest"),
        ] {
            models.push(alias.into());
            options.push((alias.into(), desc.into()));
        }
    }
    if let Some(p) = providers::find(&cfg.provider) {
        if !models.contains(&p.default_model.to_string()) {
            models.push(p.default_model.into());
            options.push((p.default_model.into(), "provider default".into()));
        }
        let tx = r.bg_tx.clone();
        let p = *p;
        std::thread::spawn(move || {
            let _ = tx.send(crate::repl::Bg::Models(
                providers::list_models(&p).map_err(|e| e.to_string()),
            ));
        });
    }
    let current = r.stats.model.clone();
    let idx = models.iter().position(|m| *m == current).unwrap_or(0);
    if !current.is_empty() && !models.contains(&current) && current != "(not configured)" {
        models.insert(0, current.clone());
        options.insert(0, (current.clone(), "current".into()));
    }
    let body = vec![
        Line::raw(format!(
            "Provider: {}. Switch between models; the choice is saved for future sessions.",
            if cfg.provider.is_empty() {
                "none"
            } else {
                &cfg.provider
            }
        )),
        Line::styled(
            "Type /model <name> to use any other model id.",
            Style::fg(r.theme.dim),
        ),
    ];
    let mut pk = picker("Select model", body, options, PickerKind::Model(models));
    pk.idx = idx;
    r.picker = Some(pk);
}

fn open_theme_picker(r: &mut Repl) {
    let options = ThemeName::all()
        .iter()
        .map(|t| (t.id().to_string(), t.describe().to_string()))
        .collect();
    let mut pk = picker(
        "Theme",
        vec![Line::raw(
            "Choose the text style that looks best with your terminal:",
        )],
        options,
        PickerKind::Theme,
    );
    pk.idx = ThemeName::all()
        .iter()
        .position(|t| *t == r.theme_name)
        .unwrap_or(0);
    r.picker = Some(pk);
}

fn config_items(r: &Repl) -> Vec<(String, String)> {
    let s = crate::settings::Settings::load(&r.root);
    let onoff = |b: bool| if b { "true" } else { "false" }.to_string();
    vec![
        ("Theme".into(), r.theme_name.id().into()),
        ("Verbose output".into(), onoff(r.verbose)),
        ("Show thinking".into(), onoff(r.show_thinking)),
        ("Auto-compact".into(), onoff(s.auto_compact)),
        ("Auto-update check".into(), onoff(s.auto_update)),
        (
            "Default permission mode".into(),
            s.default_mode.unwrap_or_else(|| "default".into()),
        ),
        ("Model".into(), r.stats.model.clone()),
    ]
}

fn open_config(r: &mut Repl) {
    let options = config_items(r);
    r.picker = Some(picker(
        "Settings",
        vec![Line::styled(
            "Enter to change · saved to ~/.ice/settings.json",
            Style::fg(r.theme.dim),
        )],
        options,
        PickerKind::Config,
    ));
}

fn config_toggle(r: &mut Repl, i: usize) {
    match i {
        0 => {
            let next = r.theme_name;
            let all = ThemeName::all();
            let n = all[(all.iter().position(|t| *t == next).unwrap_or(0) + 1) % all.len()];
            set_theme(r, n);
        }
        1 => {
            r.verbose = !r.verbose;
            let _ = crate::settings::set_user_value("verbose", serde_json::json!(r.verbose));
        }
        2 => {
            r.show_thinking = !r.show_thinking;
            let _ =
                crate::settings::set_user_value("showThinking", serde_json::json!(r.show_thinking));
        }
        3 => {
            let cur = crate::settings::Settings::load(&r.root).auto_compact;
            let _ = crate::settings::set_user_value("autoCompactEnabled", serde_json::json!(!cur));
        }
        4 => {
            let cur = crate::settings::Settings::load(&r.root).auto_update;
            let _ = crate::settings::set_user_value("autoUpdates", serde_json::json!(!cur));
        }
        5 => {
            let cur = crate::settings::Settings::load(&r.root)
                .default_mode
                .unwrap_or_else(|| "default".into());
            let next = match cur.as_str() {
                "default" => "acceptEdits",
                "acceptEdits" => "plan",
                _ => "default",
            };
            let mut v = crate::settings::user_json();
            if !v.is_object() {
                v = serde_json::json!({});
            }
            if !v["permissions"].is_object() {
                v["permissions"] = serde_json::json!({});
            }
            v["permissions"]["defaultMode"] = serde_json::json!(next);
            let _ = crate::settings::set_user_value("permissions", v["permissions"].clone());
        }
        _ => {
            r.picker = None;
            open_model_picker(r);
        }
    }
}

pub fn open_rewind(r: &mut Repl) {
    if r.running {
        return;
    }
    let prompts: Vec<(usize, String)> = r
        .items
        .iter()
        .filter_map(|i| {
            if let Item::User(t) = i {
                Some(t.clone())
            } else {
                None
            }
        })
        .filter(|t| !t.starts_with('/') && !t.starts_with('#'))
        .enumerate()
        .collect();
    if prompts.is_empty() {
        r.flash("Nothing to rewind to yet");
        return;
    }
    let options = prompts
        .iter()
        .rev()
        .map(|(_, t)| {
            (
                render::truncate(t.lines().next().unwrap_or(""), 70),
                String::new(),
            )
        })
        .collect();
    let list: Vec<(usize, String)> = prompts.into_iter().rev().collect();
    r.picker = Some(picker(
        "Rewind",
        vec![Line::raw("Restore the conversation to the point before one of your messages. The message returns to the prompt so you can edit it. (Files are not reverted.)")],
        options,
        PickerKind::Rewind(list),
    ));
}

pub fn open_memory_picker(r: &mut Repl, note: String) {
    if note.is_empty() {
        return;
    }
    let options = vec![
        ("Project memory".into(), "Checked in at ./ICE.md".into()),
        (
            "Project memory (local)".into(),
            "Gitignored in ./ICE.local.md".into(),
        ),
        (
            "User memory".into(),
            format!(
                "Saved in {}",
                crate::settings::user_dir().join("ICE.md").display()
            ),
        ),
    ];
    r.picker = Some(picker(
        "Where should this memory be saved?",
        vec![Line::raw(note.clone())],
        options,
        PickerKind::Memory(note),
    ));
}

fn open_memory_files(r: &mut Repl) {
    let options = vec![
        (
            "Project memory".into(),
            r.root.join("ICE.md").display().to_string(),
        ),
        (
            "Project memory (local)".into(),
            r.root.join("ICE.local.md").display().to_string(),
        ),
        (
            "User memory".into(),
            crate::settings::user_dir()
                .join("ICE.md")
                .display()
                .to_string(),
        ),
    ];
    r.picker = Some(picker(
        "Open memory file in your editor",
        vec![],
        options,
        PickerKind::OpenMemory,
    ));
}

fn open_bashes(r: &mut Repl) {
    let Some(ctx) = &r.ctx else { return };
    let shells = crate::tools::background_shells(ctx);
    if shells.is_empty() {
        r.flash("No background shells");
        return;
    }
    let ids = shells.iter().map(|(id, _, _)| id.clone()).collect();
    let options = shells
        .iter()
        .map(|(id, cmd, running)| {
            (
                format!("{id}: {}", render::truncate(cmd, 50)),
                if *running {
                    "running · Enter to kill".into()
                } else {
                    "completed".into()
                },
            )
        })
        .collect();
    r.picker = Some(picker(
        "Background shells",
        vec![],
        options,
        PickerKind::Bashes(ids),
    ));
}

fn open_in_editor(r: &mut Repl, path: &Path) {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| {
            if cfg!(windows) {
                "notepad".into()
            } else if crate::tools::which("nano") {
                "nano".into()
            } else {
                "vi".into()
            }
        });
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = crossterm::terminal::disable_raw_mode();
    let mut parts = editor.split_whitespace();
    let prog = parts.next().unwrap_or("vi").to_string();
    let status = std::process::Command::new(prog)
        .args(parts)
        .arg(path)
        .status();
    let _ = crossterm::terminal::enable_raw_mode();
    r.redraw_all();
    r.items.push(Item::Info(match status {
        Ok(_) => format!("Opened memory file at {}", path.display()),
        Err(e) => format!("Could not open editor `{editor}`: {e}. Set $EDITOR."),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_commands_expand_arguments() {
        let root = std::env::temp_dir().join(format!("ice-cmd-{}", crate::conv::new_id("c")));
        std::fs::create_dir_all(root.join(".ice/commands/git")).unwrap();
        std::fs::write(
            root.join(".ice/commands/fix-issue.md"),
            "---\ndescription: Fix a GitHub issue\n---\nFix issue #$ARGUMENTS now",
        )
        .unwrap();
        std::fs::write(root.join(".ice/commands/git/sync.md"), "Pull and rebase").unwrap();
        let cmds = custom_commands(&root);
        let fix = cmds.iter().find(|(n, _, _)| n == "fix-issue").unwrap();
        assert!(fix.1.starts_with("Fix a GitHub issue"));
        assert!(cmds.iter().any(|(n, _, _)| n == "git:sync"));
        assert!(all_commands(&root).iter().any(|(n, _)| n == "compact"));
    }

    #[test]
    fn reports_render() {
        let s = Stats {
            usage: crate::conv::Usage {
                input_tokens: 1500,
                output_tokens: 200,
                ..Default::default()
            },
            cost: Some(0.0123),
            api_ms: 1500,
            wall_ms: 65_000,
            lines_added: 3,
            ..Default::default()
        };
        let c = cost_report(&s);
        assert!(c.contains("$0.0123") && c.contains("1m 5s") && c.contains("3 lines added"));
        let ctx = context_report(&[("Messages".into(), 20_000)], 0, 200_000, "m");
        assert!(ctx.contains("10%"));
    }
}
