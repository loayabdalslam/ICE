//! The built-in tool set, shaped like Claude Code's: same names, same
//! parameters, same error messages, same output formats — so models trained
//! on those tools use them fluently.

use crate::model::ToolSpec;
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

pub const READ_DEFAULT_LIMIT: usize = 2000;
pub const READ_MAX_LINE: usize = 2000;
pub const BASH_MAX_OUTPUT: usize = 30_000;
pub const BASH_DEFAULT_TIMEOUT_MS: u64 = 120_000;
pub const BASH_MAX_TIMEOUT_MS: u64 = 600_000;
const READ_MAX_BYTES: u64 = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    pub content: String,
    pub status: String,
    #[serde(default, rename = "activeForm")]
    pub active_form: String,
}

/// What the UI shows for a finished tool call.
#[derive(Clone, Debug)]
pub enum Display {
    /// A one-line summary plus optional output lines.
    Lines {
        summary: String,
        body: Vec<String>,
    },
    /// A file change: summary + unified diff lines ("+", "-", " " prefixed,
    /// with line numbers).
    Diff {
        summary: String,
        lines: Vec<DiffLine>,
    },
    Todos(Vec<TodoItem>),
}

#[derive(Clone, Debug)]
pub struct DiffLine {
    pub kind: char,
    pub lineno: usize,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct ToolOutput {
    /// Text returned to the model.
    pub content: String,
    pub is_error: bool,
    pub display: Display,
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>, display: Display) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            display,
        }
    }

    pub fn err(msg: impl Into<String>) -> Self {
        let m = msg.into();
        Self {
            content: m.clone(),
            is_error: true,
            display: Display::Lines {
                summary: format!("Error: {}", m.lines().next().unwrap_or("")),
                body: m.lines().skip(1).take(8).map(String::from).collect(),
            },
        }
    }
}

struct BgShell {
    command: String,
    child: Child,
    output: Arc<Mutex<String>>,
    read_upto: usize,
    started: Instant,
}

/// Per-session tool state.
pub struct ToolCtx {
    pub root: PathBuf,
    pub cwd: Mutex<PathBuf>,
    /// file → mtime when last read/written by the agent (read-before-write).
    pub read_state: Mutex<HashMap<PathBuf, Option<SystemTime>>>,
    pub todos: Mutex<Vec<TodoItem>>,
    pub cancel: Arc<AtomicBool>,
    bg: Mutex<HashMap<String, BgShell>>,
    bg_seq: Mutex<u32>,
    fetch_cache: Mutex<HashMap<String, (Instant, String)>>,
}

impl ToolCtx {
    pub fn new(root: PathBuf, cancel: Arc<AtomicBool>) -> Self {
        Self {
            cwd: Mutex::new(root.clone()),
            root,
            read_state: Mutex::new(HashMap::new()),
            todos: Mutex::new(Vec::new()),
            cancel,
            bg: Mutex::new(HashMap::new()),
            bg_seq: Mutex::new(0),
            fetch_cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Resolve a model-supplied path: absolute stays, relative is taken from
    /// the project root; `~` expands; `.`/`..` are normalised lexically.
    pub fn resolve(&self, p: &str) -> PathBuf {
        let p = p.trim();
        let raw = if let Some(rest) = p.strip_prefix("~/") {
            home_dir().join(rest)
        } else {
            PathBuf::from(p)
        };
        let joined = if raw.is_absolute() {
            raw
        } else {
            self.root.join(raw)
        };
        normalize(&joined)
    }

    /// True when `p` is inside the project — resolving symlinks, so a link
    /// pointing outside the project doesn't count as inside.
    pub fn inside_root(&self, p: &Path) -> bool {
        let root = normalize(&self.root);
        normalize(p).starts_with(&root)
            && crate::sandbox::resolve(&self.root, &p.display().to_string()).is_ok()
    }

    fn mark_read(&self, p: &Path) {
        let m = fs::metadata(p).and_then(|m| m.modified()).ok();
        self.read_state.lock().unwrap().insert(p.to_path_buf(), m);
    }

    /// Enforce read-before-write and detect external modification.
    fn check_fresh(&self, p: &Path) -> Result<(), String> {
        if !p.exists() {
            return Ok(());
        }
        let st = self.read_state.lock().unwrap();
        match st.get(p) {
            None => Err("File has not been read yet. Read it first before writing to it.".into()),
            Some(seen) => {
                let now = fs::metadata(p).and_then(|m| m.modified()).ok();
                if now.is_some() && seen.is_some() && now > *seen {
                    Err("File has been modified since read, either by the user or by a linter. Read it again before attempting to write it.".into())
                } else {
                    Ok(())
                }
            }
        }
    }

    pub fn kill_background(&self) {
        let mut bg = self.bg.lock().unwrap();
        for (_, s) in bg.iter_mut() {
            kill_tree(&mut s.child);
        }
        bg.clear();
    }
}

impl Drop for ToolCtx {
    fn drop(&mut self) {
        self.kill_background();
    }
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Path as shown to the user: relative to the root when inside it.
pub fn display_path(ctx: &ToolCtx, p: &Path) -> String {
    let root = normalize(&ctx.root);
    match normalize(p).strip_prefix(&root) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.display().to_string(),
        Ok(_) => ".".into(),
        Err(_) => p.display().to_string(),
    }
}

// ─────────────────────────── specs ───────────────────────────

pub const READ_ONLY_TOOLS: &[&str] = &[
    "Read",
    "Glob",
    "Grep",
    "LS",
    "WebSearch",
    "WebFetch",
    "TodoWrite",
    "Task",
    "BashOutput",
    "ExitPlanMode",
    "Skill",
];
pub const EDIT_TOOLS: &[&str] = &["Edit", "MultiEdit", "Write"];

pub fn is_read_only(name: &str) -> bool {
    READ_ONLY_TOOLS.contains(&name)
}

/// Map loose names models sometimes emit (read_file, bash, str_replace…) to
/// the canonical tool names.
pub fn canonical_name(raw: &str) -> Option<String> {
    let n = raw.trim();
    if n.starts_with("mcp__") {
        return Some(n.to_string());
    }
    let low = n.to_ascii_lowercase().replace(['-', ' '], "_");
    let c = match low.as_str() {
        "read" | "read_file" | "view" | "cat" | "open_file" => "Read",
        "write" | "write_file" | "create_file" | "create" => "Write",
        "edit" | "edit_file" | "str_replace" | "replace" | "str_replace_editor" | "patch" => "Edit",
        "multiedit" | "multi_edit" => "MultiEdit",
        "glob" | "find_files" => "Glob",
        "grep" | "search" | "search_files" | "rg" => "Grep",
        "ls" | "list" | "list_dir" | "list_directory" | "list_files" => "LS",
        "bash" | "run" | "shell" | "exec" | "execute" | "run_command" | "terminal" => "Bash",
        "bashoutput" | "bash_output" => "BashOutput",
        "killshell" | "kill_shell" | "killbash" => "KillShell",
        "webfetch" | "web_fetch" | "fetch" => "WebFetch",
        "websearch" | "web_search" => "WebSearch",
        "todowrite" | "todo_write" | "todos" | "todo" => "TodoWrite",
        "task" | "agent" | "subagent" => "Task",
        "exitplanmode" | "exit_plan_mode" => "ExitPlanMode",
        "skill" => "Skill",
        _ => return None,
    };
    Some(c.to_string())
}

fn spec(name: &str, description: &str, schema: Value) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: description.trim().into(),
        input_schema: schema,
    }
}

pub fn builtin_specs(skills: &[String]) -> Vec<ToolSpec> {
    let mut v = vec![
        spec(
            "Task",
            r#"Launch a new agent to handle complex, multi-step tasks autonomously.

Available agent types:
- general-purpose: researching complex questions, searching for code, and executing multi-step tasks. (Tools: *)
- Explore: fast read-only agent for exploring codebases — find files, search code, answer questions about the codebase. (Tools: Read, Glob, Grep, LS, WebFetch, WebSearch)
- Plan: read-only software architect that designs an implementation plan and identifies critical files. (Tools: Read, Glob, Grep, LS)

When to use: open-ended searches that may take several rounds, or independent work that can run in parallel (launch several agents in ONE message).
When NOT to use: reading a specific file path (use Read), finding a class definition (use Glob/Grep), searching within 2-3 files.

Notes:
- The agent's result is returned to you, not shown to the user — summarise it for them.
- Each invocation is stateless: give a detailed, self-contained prompt and say exactly what to return.
- Tell the agent whether to write code or only research."#,
            json!({"type":"object","properties":{
                "description":{"type":"string","description":"A short (3-5 word) description of the task"},
                "prompt":{"type":"string","description":"The task for the agent to perform"},
                "subagent_type":{"type":"string","description":"The type of specialized agent to use for this task"}
            },"required":["description","prompt","subagent_type"]}),
        ),
        spec(
            "Bash",
            r#"Executes a bash command in a persistent shell session with optional timeout.

- The working directory persists between commands; prefer absolute paths and avoid `cd` unless the user asks.
- Always quote paths that contain spaces with double quotes.
- Timeout defaults to 120000ms (2 minutes); max 600000ms (10 minutes).
- Output longer than 30000 characters is truncated.
- Use `run_in_background` for long-running processes (servers, watchers); read output later with BashOutput.
- Do NOT use find, grep, cat, head, tail, sed, awk or echo for file operations — use Glob, Grep, Read, Edit and Write instead.
- Chain dependent commands with `&&`; issue independent commands as separate parallel tool calls.
- Never use interactive flags (e.g. `git rebase -i`, `git add -i`).

Git: only commit when the user asks. Never update git config, never skip hooks (--no-verify), never force-push to main/master. To commit: run git status, git diff and git log in parallel, draft a concise message focused on the "why", then add relevant files and commit with a HEREDOC message."#,
            json!({"type":"object","properties":{
                "command":{"type":"string","description":"The command to execute"},
                "timeout":{"type":"number","description":"Optional timeout in milliseconds (max 600000)"},
                "description":{"type":"string","description":"Clear, concise description of what this command does in 5-10 words, in active voice"},
                "run_in_background":{"type":"boolean","description":"Set to true to run this command in the background. Use BashOutput to read the output later."}
            },"required":["command"]}),
        ),
        spec(
            "BashOutput",
            "Retrieves new output from a running or completed background bash shell. Returns only output produced since the last check, plus the shell status.",
            json!({"type":"object","properties":{
                "bash_id":{"type":"string","description":"The ID of the background shell to retrieve output from"},
                "filter":{"type":"string","description":"Optional regular expression; only matching lines are returned"}
            },"required":["bash_id"]}),
        ),
        spec(
            "KillShell",
            "Kills a running background bash shell by its ID.",
            json!({"type":"object","properties":{
                "shell_id":{"type":"string","description":"The ID of the background shell to kill"}
            },"required":["shell_id"]}),
        ),
        spec(
            "Glob",
            r#"Fast file pattern matching tool that works with any codebase size.
- Supports glob patterns like "**/*.js" or "src/**/*.ts"
- Returns matching file paths sorted by modification time (newest first)
- Respects .gitignore
- Use this tool when you need to find files by name patterns"#,
            json!({"type":"object","properties":{
                "pattern":{"type":"string","description":"The glob pattern to match files against"},
                "path":{"type":"string","description":"The directory to search in. Omit to use the current working directory."}
            },"required":["pattern"]}),
        ),
        spec(
            "Grep",
            r#"A powerful search tool built on regular expressions (ripgrep-style).
- ALWAYS use Grep for search tasks. NEVER invoke `grep` or `rg` as a Bash command.
- Supports full regex syntax (e.g. "log.*Error", "function\s+\w+")
- Filter files with the glob parameter (e.g. "*.js", "**/*.tsx") or type parameter (e.g. "js", "py", "rust")
- Output modes: "content" shows matching lines, "files_with_matches" shows only file paths (default), "count" shows match counts
- Pattern syntax: literal braces need escaping (use `interface\{\}` to find `interface{}`)
- Multiline matching: by default patterns match within single lines only; use multiline: true for cross-line patterns"#,
            json!({"type":"object","properties":{
                "pattern":{"type":"string","description":"The regular expression pattern to search for in file contents"},
                "path":{"type":"string","description":"File or directory to search in. Defaults to current working directory."},
                "glob":{"type":"string","description":"Glob pattern to filter files (e.g. \"*.js\", \"*.{ts,tsx}\")"},
                "type":{"type":"string","description":"File type to search (e.g. js, py, rust, go, java)"},
                "output_mode":{"type":"string","enum":["content","files_with_matches","count"],"description":"Output mode. Defaults to \"files_with_matches\"."},
                "-i":{"type":"boolean","description":"Case insensitive search"},
                "-n":{"type":"boolean","description":"Show line numbers in output (content mode). Defaults to true."},
                "-A":{"type":"number","description":"Number of lines to show after each match (content mode)"},
                "-B":{"type":"number","description":"Number of lines to show before each match (content mode)"},
                "-C":{"type":"number","description":"Number of lines to show before and after each match (content mode)"},
                "head_limit":{"type":"number","description":"Limit output to first N lines/entries"},
                "multiline":{"type":"boolean","description":"Enable multiline mode where . matches newlines and patterns can span lines"}
            },"required":["pattern"]}),
        ),
        spec(
            "LS",
            "Lists files and directories in a given path. The path parameter should be an absolute path or relative to the project root. You can optionally provide an array of glob patterns to ignore. Prefer Glob and Grep when you know what you are looking for.",
            json!({"type":"object","properties":{
                "path":{"type":"string","description":"The path to the directory to list"},
                "ignore":{"type":"array","items":{"type":"string"},"description":"List of glob patterns to ignore"}
            },"required":["path"]}),
        ),
        spec(
            "ExitPlanMode",
            "Use this tool when you are in plan mode and have finished planning the implementation steps of a task that requires writing code. Present the plan (markdown) for the user to approve. Do NOT use it for research-only tasks.",
            json!({"type":"object","properties":{
                "plan":{"type":"string","description":"The plan you came up with, that you want to run by the user for approval. Supports markdown."}
            },"required":["plan"]}),
        ),
        spec(
            "Read",
            r#"Reads a file from the local filesystem.

- The file_path parameter may be absolute or relative to the project root.
- By default, it reads up to 2000 lines starting from the beginning of the file.
- You can optionally specify a line offset and limit (useful for long files), but it's recommended to read the whole file.
- Any lines longer than 2000 characters will be truncated.
- Results are returned using cat -n format, with line numbers starting at 1.
- It is always better to speculatively read multiple potentially useful files in parallel in one response.
- If you read a file that exists but has empty contents you will receive a system reminder warning in place of file contents."#,
            json!({"type":"object","properties":{
                "file_path":{"type":"string","description":"The path to the file to read"},
                "offset":{"type":"number","description":"The line number to start reading from. Only provide if the file is too large to read at once"},
                "limit":{"type":"number","description":"The number of lines to read. Only provide if the file is too large to read at once."}
            },"required":["file_path"]}),
        ),
        spec(
            "Edit",
            r#"Performs exact string replacements in files.

Usage:
- You must use your Read tool at least once in the conversation before editing. This tool will error if you attempt an edit without reading the file.
- When editing text from Read tool output, preserve the exact indentation (tabs/spaces) as it appears AFTER the line number prefix. Never include any part of the line number prefix in old_string or new_string.
- ALWAYS prefer editing existing files. NEVER write new files unless explicitly required.
- The edit will FAIL if old_string is not unique in the file. Either provide a larger string with more surrounding context to make it unique or use replace_all to change every instance.
- Use replace_all for replacing and renaming strings across the file."#,
            json!({"type":"object","properties":{
                "file_path":{"type":"string","description":"The path to the file to modify"},
                "old_string":{"type":"string","description":"The text to replace"},
                "new_string":{"type":"string","description":"The text to replace it with (must be different from old_string)"},
                "replace_all":{"type":"boolean","default":false,"description":"Replace all occurrences of old_string (default false)"}
            },"required":["file_path","old_string","new_string"]}),
        ),
        spec(
            "MultiEdit",
            "Makes multiple edits to a single file in one atomic operation. Edits are applied in sequence, each operating on the result of the previous one; if any edit fails, none are applied. Same rules as Edit (read the file first, old_string must be unique unless replace_all).",
            json!({"type":"object","properties":{
                "file_path":{"type":"string","description":"The path to the file to modify"},
                "edits":{"type":"array","items":{"type":"object","properties":{
                    "old_string":{"type":"string"},
                    "new_string":{"type":"string"},
                    "replace_all":{"type":"boolean","default":false}
                },"required":["old_string","new_string"]},"minItems":1,"description":"Array of edit operations to perform sequentially on the file"}
            },"required":["file_path","edits"]}),
        ),
        spec(
            "Write",
            r#"Writes a file to the local filesystem.

- This tool will overwrite the existing file if there is one at the provided path.
- If this is an existing file, you MUST use the Read tool first to read the file's contents. This tool will fail if you did not read the file first.
- ALWAYS prefer editing existing files in the codebase. NEVER write new files unless explicitly required.
- NEVER proactively create documentation files (*.md) or README files unless explicitly requested."#,
            json!({"type":"object","properties":{
                "file_path":{"type":"string","description":"The path to the file to write"},
                "content":{"type":"string","description":"The content to write to the file"}
            },"required":["file_path","content"]}),
        ),
        spec(
            "WebFetch",
            r#"Fetches content from a URL and processes it with a prompt.
- Takes a URL and a prompt; fetches the page, converts HTML to text, and answers the prompt about it.
- HTTP URLs are upgraded to HTTPS. Results are cached for 15 minutes.
- The URL must be fully-formed and valid."#,
            json!({"type":"object","properties":{
                "url":{"type":"string","format":"uri","description":"The URL to fetch content from"},
                "prompt":{"type":"string","description":"The prompt to run on the fetched content"}
            },"required":["url","prompt"]}),
        ),
        spec(
            "TodoWrite",
            r#"Create and manage a structured task list for the current session. It helps you track progress, organize complex tasks, and shows the user what you're doing.

Use it proactively for complex multi-step tasks (3+ steps), when the user gives a list of things to do, or right after receiving new instructions. Skip it for single, trivial tasks.

Rules:
- Exactly ONE task should be in_progress at any time.
- Mark tasks completed IMMEDIATELY after finishing them; don't batch completions.
- Only mark a task completed when it is fully done (tests pass, no unresolved errors).
- Each task has `content` (imperative, e.g. "Run tests") and `activeForm` (present continuous, e.g. "Running tests")."#,
            json!({"type":"object","properties":{
                "todos":{"type":"array","items":{"type":"object","properties":{
                    "content":{"type":"string","minLength":1},
                    "status":{"type":"string","enum":["pending","in_progress","completed"]},
                    "activeForm":{"type":"string","minLength":1}
                },"required":["content","status","activeForm"]},"description":"The updated todo list"}
            },"required":["todos"]}),
        ),
        spec(
            "WebSearch",
            "Searches the web and returns result titles, URLs and snippets. Use it for information beyond your knowledge cutoff or about current events. After answering, include a \"Sources:\" list of the relevant URLs as markdown links.",
            json!({"type":"object","properties":{
                "query":{"type":"string","minLength":2,"description":"The search query to use"},
                "allowed_domains":{"type":"array","items":{"type":"string"},"description":"Only include search results from these domains"},
                "blocked_domains":{"type":"array","items":{"type":"string"},"description":"Never include search results from these domains"}
            },"required":["query"]}),
        ),
    ];
    if !skills.is_empty() {
        v.push(spec(
            "Skill",
            &format!(
                "Load a skill — a packaged set of instructions for a particular kind of task. When a task matches a skill, call this first and follow the returned instructions.\n\nAvailable skills:\n{}",
                skills.join("\n")
            ),
            json!({"type":"object","properties":{
                "skill":{"type":"string","description":"The skill name"}
            },"required":["skill"]}),
        ));
    }
    v
}

// ─────────────────────────── dispatch ───────────────────────────

fn s<'a>(input: &'a Value, k: &str) -> Option<&'a str> {
    input.get(k).and_then(|v| v.as_str())
}

fn n(input: &Value, k: &str) -> Option<u64> {
    input.get(k).and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_f64().map(|f| f as u64))
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
    })
}

fn b(input: &Value, k: &str) -> bool {
    input
        .get(k)
        .map(|v| v.as_bool().unwrap_or_else(|| v.as_str() == Some("true")))
        .unwrap_or(false)
}

fn need<'a>(input: &'a Value, k: &str) -> Result<&'a str, ToolOutput> {
    s(input, k).ok_or_else(|| {
        ToolOutput::err(format!(
            "InputValidationError: The required parameter `{k}` is missing"
        ))
    })
}

/// Run a built-in tool. (Task, ExitPlanMode and MCP tools are dispatched by
/// the engine.)
pub fn run(ctx: &ToolCtx, name: &str, input: &Value) -> ToolOutput {
    let r = match name {
        "Read" => read(ctx, input),
        "Write" => write(ctx, input),
        "Edit" => edit(ctx, input),
        "MultiEdit" => multi_edit(ctx, input),
        "Glob" => glob(ctx, input),
        "Grep" => grep(ctx, input),
        "LS" => ls(ctx, input),
        "Bash" => bash(ctx, input),
        "BashOutput" => bash_output(ctx, input),
        "KillShell" => kill_shell(ctx, input),
        "TodoWrite" => todo_write(ctx, input),
        "WebSearch" => web_search(ctx, input),
        "Skill" => skill(ctx, input),
        other => Err(ToolOutput::err(format!(
            "Error: No such tool available: {other}"
        ))),
    };
    r.unwrap_or_else(|e| e)
}

type TR = Result<ToolOutput, ToolOutput>;

// ─────────────────────────── Read ───────────────────────────

fn read(ctx: &ToolCtx, input: &Value) -> TR {
    let fp = need(input, "file_path")?;
    let p = ctx.resolve(fp);
    if p.is_dir() {
        return Err(ToolOutput::err(
            "EISDIR: illegal operation on a directory, read",
        ));
    }
    if !p.exists() {
        let mut msg = "File does not exist.".to_string();
        if let Some(sug) = similar_file(&p) {
            msg.push_str(&format!(" Did you mean {}?", display_path(ctx, &sug)));
        }
        return Err(ToolOutput::err(msg));
    }
    let meta = fs::metadata(&p).map_err(|e| ToolOutput::err(e.to_string()))?;
    let offset = n(input, "offset").unwrap_or(1).max(1) as usize;
    let explicit_limit = n(input, "limit").map(|l| l as usize);
    if meta.len() > READ_MAX_BYTES && explicit_limit.is_none() && offset <= 1 {
        return Err(ToolOutput::err(format!(
            "File content ({}KB) exceeds maximum allowed size ({}KB). Please use offset and limit parameters to read specific portions of the file, or use the Grep tool to search for specific content.",
            meta.len() / 1024,
            READ_MAX_BYTES / 1024
        )));
    }
    let mut bytes = Vec::new();
    fs::File::open(&p)
        .and_then(|mut f| f.read_to_end(&mut bytes))
        .map_err(|e| ToolOutput::err(e.to_string()))?;
    if looks_binary(&bytes) {
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        return Err(ToolOutput::err(format!(
            "This tool cannot read binary files. The file appears to be a binary .{ext} file. Please use appropriate tools for binary file analysis."
        )));
    }
    let text = String::from_utf8_lossy(&bytes);
    ctx.mark_read(&p);
    if text.is_empty() {
        return Ok(ToolOutput::ok(
            "<system-reminder>Warning: the file exists but the contents are empty.</system-reminder>",
            Display::Lines { summary: "Read 0 lines (empty file)".into(), body: vec![] },
        ));
    }
    let limit = explicit_limit.unwrap_or(READ_DEFAULT_LIMIT);
    let lines: Vec<&str> = text.split('\n').collect();
    let total = if text.ends_with('\n') {
        lines.len() - 1
    } else {
        lines.len()
    };
    if offset > total {
        return Ok(ToolOutput::ok(
            format!("<system-reminder>Warning: the file exists but is shorter than the provided offset ({offset}). The file has {total} lines.</system-reminder>"),
            Display::Lines { summary: "Read 0 lines".into(), body: vec![] },
        ));
    }
    let end = (offset - 1 + limit).min(total);
    let out = cat_n(&lines[offset - 1..end], offset);
    let shown = end - (offset - 1);
    let mut content = out;
    if end < total && explicit_limit.is_none() {
        content.push_str(&format!(
            "\n\n<system-reminder>Showing lines {offset}-{end} of {total}. Use offset/limit to read more.</system-reminder>"
        ));
    }
    Ok(ToolOutput::ok(
        content,
        Display::Lines {
            summary: format!("Read {shown} line{}", plural(shown)),
            body: vec![],
        },
    ))
}

/// `cat -n` style: right-aligned line number (width 6), a tab, the line.
pub fn cat_n(lines: &[&str], start: usize) -> String {
    let mut out = String::new();
    for (i, l) in lines.iter().enumerate() {
        let l = l.strip_suffix('\r').unwrap_or(l);
        let l: String = if l.chars().count() > READ_MAX_LINE {
            let mut t: String = l.chars().take(READ_MAX_LINE).collect();
            t.push_str("… [line truncated]");
            t
        } else {
            l.to_string()
        };
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&format!("{:>6}\t{}", start + i, l));
    }
    out
}

fn looks_binary(b: &[u8]) -> bool {
    let head = &b[..b.len().min(8000)];
    head.contains(&0)
}

fn similar_file(p: &Path) -> Option<PathBuf> {
    let parent = p.parent()?;
    let stem = p.file_stem()?.to_str()?.to_ascii_lowercase();
    let name = p.file_name()?.to_str()?.to_ascii_lowercase();
    let rd = fs::read_dir(parent).ok()?;
    for e in rd.flatten() {
        let q = e.path();
        let qn = q.file_name()?.to_str()?.to_ascii_lowercase();
        let qs = q
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if qn != name && qs == stem {
            return Some(q);
        }
    }
    None
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

// ─────────────────────────── Write / Edit ───────────────────────────

fn write(ctx: &ToolCtx, input: &Value) -> TR {
    let fp = need(input, "file_path")?;
    let content = s(input, "content").ok_or_else(|| {
        ToolOutput::err("InputValidationError: The required parameter `content` is missing")
    })?;
    let p = ctx.resolve(fp);
    if p.is_dir() {
        return Err(ToolOutput::err(
            "EISDIR: illegal operation on a directory, write",
        ));
    }
    ctx.check_fresh(&p).map_err(ToolOutput::err)?;
    let old = fs::read_to_string(&p).ok();
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).map_err(|e| ToolOutput::err(e.to_string()))?;
    }
    // Preserve CRLF files' line endings on overwrite.
    let to_write = match &old {
        Some(o) if o.contains("\r\n") && !content.contains("\r\n") => content.replace('\n', "\r\n"),
        _ => content.to_string(),
    };
    fs::write(&p, &to_write).map_err(|e| ToolOutput::err(e.to_string()))?;
    ctx.mark_read(&p);
    let dp = display_path(ctx, &p);
    match old {
        None => {
            let lines: Vec<&str> = content.lines().collect();
            let n = lines.len();
            Ok(ToolOutput::ok(
                format!("File created successfully at: {}", p.display()),
                Display::Diff {
                    summary: format!("Wrote {n} line{} to {dp}", plural(n)),
                    lines: lines
                        .iter()
                        .enumerate()
                        .map(|(i, l)| DiffLine {
                            kind: '+',
                            lineno: i + 1,
                            text: l.to_string(),
                        })
                        .collect(),
                },
            ))
        }
        Some(o) => {
            let o = o.replace("\r\n", "\n");
            let (lines, adds, dels) = diff_lines(&o, content);
            Ok(ToolOutput::ok(
                format!(
                    "The file {} has been updated. Here's the result of running `cat -n` on a snippet of the edited file:\n{}",
                    p.display(),
                    cat_n(&content.lines().take(40).collect::<Vec<_>>(), 1)
                ),
                Display::Diff { summary: change_summary(&dp, adds, dels), lines },
            ))
        }
    }
}

fn change_summary(dp: &str, adds: usize, dels: usize) -> String {
    let mut parts = Vec::new();
    if adds > 0 {
        parts.push(format!("{adds} addition{}", plural(adds)));
    }
    if dels > 0 {
        parts.push(format!("{dels} removal{}", plural(dels)));
    }
    if parts.is_empty() {
        format!("Updated {dp} (no line changes)")
    } else {
        format!("Updated {dp} with {}", parts.join(" and "))
    }
}

/// Apply one replacement to LF-normalised text, with Claude Code's errors.
fn apply_edit(
    text: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Result<(String, usize), String> {
    if old == new {
        return Err("No changes to make: old_string and new_string are exactly the same.".into());
    }
    let count = text.matches(old).count();
    if count == 0 {
        // Tolerate a trailing-whitespace mismatch on the last line of old.
        return Err(format!(
            "String to replace not found in file.\nString: {old}"
        ));
    }
    if count > 1 && !replace_all {
        return Err(format!(
            "Found {count} matches of the string to replace, but replace_all is false. To replace all occurrences, set replace_all to true. To replace only one occurrence, please provide more context to uniquely identify the instance.\nString: {old}"
        ));
    }
    let pos = text.find(old).unwrap_or(0);
    let out = if replace_all {
        text.replace(old, new)
    } else {
        text.replacen(old, new, 1)
    };
    Ok((out, pos))
}

fn edit(ctx: &ToolCtx, input: &Value) -> TR {
    let fp = need(input, "file_path")?;
    let old = s(input, "old_string").ok_or_else(|| {
        ToolOutput::err("InputValidationError: The required parameter `old_string` is missing")
    })?;
    let new = s(input, "new_string").ok_or_else(|| {
        ToolOutput::err("InputValidationError: The required parameter `new_string` is missing")
    })?;
    let edits = vec![(old.to_string(), new.to_string(), b(input, "replace_all"))];
    apply_edits(ctx, fp, &edits)
}

fn multi_edit(ctx: &ToolCtx, input: &Value) -> TR {
    let fp = need(input, "file_path")?;
    let arr = input
        .get("edits")
        .and_then(|e| e.as_array())
        .ok_or_else(|| {
            ToolOutput::err("InputValidationError: The required parameter `edits` is missing")
        })?;
    let mut edits = Vec::new();
    for e in arr {
        edits.push((
            s(e, "old_string").unwrap_or("").to_string(),
            s(e, "new_string").unwrap_or("").to_string(),
            b(e, "replace_all"),
        ));
    }
    if edits.is_empty() {
        return Err(ToolOutput::err(
            "InputValidationError: edits must contain at least one edit",
        ));
    }
    apply_edits(ctx, fp, &edits)
}

fn apply_edits(ctx: &ToolCtx, fp: &str, edits: &[(String, String, bool)]) -> TR {
    let p = ctx.resolve(fp);
    // Creating a new file: first edit with an empty old_string.
    if !p.exists() {
        if edits[0].0.is_empty() {
            let content = edits[0].1.clone();
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).map_err(|e| ToolOutput::err(e.to_string()))?;
            }
            fs::write(&p, &content).map_err(|e| ToolOutput::err(e.to_string()))?;
            ctx.mark_read(&p);
            let n = content.lines().count();
            return Ok(ToolOutput::ok(
                format!("File created successfully at: {}", p.display()),
                Display::Diff {
                    summary: format!("Wrote {n} line{} to {}", plural(n), display_path(ctx, &p)),
                    lines: content
                        .lines()
                        .enumerate()
                        .map(|(i, l)| DiffLine {
                            kind: '+',
                            lineno: i + 1,
                            text: l.into(),
                        })
                        .collect(),
                },
            ));
        }
        let mut msg = "File does not exist.".to_string();
        if let Some(sug) = similar_file(&p) {
            msg.push_str(&format!(" Did you mean {}?", display_path(ctx, &sug)));
        }
        return Err(ToolOutput::err(msg));
    }
    if p.is_dir() {
        return Err(ToolOutput::err("EISDIR: illegal operation on a directory"));
    }
    ctx.check_fresh(&p).map_err(ToolOutput::err)?;
    let raw = fs::read_to_string(&p).map_err(|e| ToolOutput::err(e.to_string()))?;
    let crlf = raw.contains("\r\n");
    let original = if crlf {
        raw.replace("\r\n", "\n")
    } else {
        raw.clone()
    };
    let mut text = original.clone();
    let mut first_pos = usize::MAX;
    for (i, (old, new, all)) in edits.iter().enumerate() {
        let old_n = old.replace("\r\n", "\n");
        let new_n = new.replace("\r\n", "\n");
        if old_n.is_empty() {
            return Err(ToolOutput::err("Cannot edit an existing file with an empty old_string. Use Write to replace its contents."));
        }
        match apply_edit(&text, &old_n, &new_n, *all) {
            Ok((t, pos)) => {
                text = t;
                first_pos = first_pos.min(pos);
            }
            Err(e) => {
                let prefix = if edits.len() > 1 {
                    format!("Edit {} failed: ", i + 1)
                } else {
                    String::new()
                };
                return Err(ToolOutput::err(format!("{prefix}{e}")));
            }
        }
    }
    let on_disk = if crlf {
        text.replace('\n', "\r\n")
    } else {
        text.clone()
    };
    fs::write(&p, on_disk).map_err(|e| ToolOutput::err(e.to_string()))?;
    ctx.mark_read(&p);
    let (lines, adds, dels) = diff_lines(&original, &text);
    let dp = display_path(ctx, &p);
    Ok(ToolOutput::ok(
        format!(
            "The file {} has been updated. Here's the result of running `cat -n` on a snippet of the edited file:\n{}",
            p.display(),
            snippet_around(&text, first_pos, 4)
        ),
        Display::Diff { summary: change_summary(&dp, adds, dels), lines },
    ))
}

/// `cat -n` of the lines around byte offset `pos` (±`ctx_lines`).
fn snippet_around(text: &str, pos: usize, ctx_lines: usize) -> String {
    let pos = pos.min(text.len());
    let line_idx = text[..pos].matches('\n').count();
    let lines: Vec<&str> = text.split('\n').collect();
    let start = line_idx.saturating_sub(ctx_lines);
    let end = (line_idx + ctx_lines + 8).min(lines.len());
    cat_n(&lines[start..end], start + 1)
}

/// A compact unified diff (changed hunks with 3 lines of context) plus
/// addition/removal counts.
pub fn diff_lines(old: &str, new: &str) -> (Vec<DiffLine>, usize, usize) {
    use similar::{ChangeTag, TextDiff};
    let diff = TextDiff::from_lines(old, new);
    let mut out = Vec::new();
    let (mut adds, mut dels) = (0, 0);
    for (gi, group) in diff.grouped_ops(3).iter().enumerate() {
        if gi > 0 {
            out.push(DiffLine {
                kind: '…',
                lineno: 0,
                text: String::new(),
            });
        }
        for op in group {
            for ch in diff.iter_changes(op) {
                let text = ch.value().trim_end_matches(['\n', '\r']).to_string();
                match ch.tag() {
                    ChangeTag::Delete => {
                        dels += 1;
                        out.push(DiffLine {
                            kind: '-',
                            lineno: ch.old_index().map(|i| i + 1).unwrap_or(0),
                            text,
                        });
                    }
                    ChangeTag::Insert => {
                        adds += 1;
                        out.push(DiffLine {
                            kind: '+',
                            lineno: ch.new_index().map(|i| i + 1).unwrap_or(0),
                            text,
                        });
                    }
                    ChangeTag::Equal => {
                        out.push(DiffLine {
                            kind: ' ',
                            lineno: ch.new_index().map(|i| i + 1).unwrap_or(0),
                            text,
                        });
                    }
                }
            }
        }
    }
    (out, adds, dels)
}

/// The diff an Edit/MultiEdit/Write *would* produce, for permission prompts.
pub fn preview(ctx: &ToolCtx, name: &str, input: &Value) -> Option<(String, Vec<DiffLine>)> {
    let p = ctx.resolve(s(input, "file_path")?);
    let dp = display_path(ctx, &p);
    let old = fs::read_to_string(&p).ok().map(|t| t.replace("\r\n", "\n"));
    let new = match name {
        "Write" => s(input, "content")?.to_string(),
        "Edit" | "MultiEdit" => {
            let edits: Vec<(String, String, bool)> = if name == "Edit" {
                vec![(
                    s(input, "old_string")?.into(),
                    s(input, "new_string")?.into(),
                    b(input, "replace_all"),
                )]
            } else {
                input
                    .get("edits")?
                    .as_array()?
                    .iter()
                    .map(|e| {
                        (
                            s(e, "old_string").unwrap_or("").into(),
                            s(e, "new_string").unwrap_or("").into(),
                            b(e, "replace_all"),
                        )
                    })
                    .collect()
            };
            match &old {
                None if edits.first().map(|e| e.0.is_empty()).unwrap_or(false) => {
                    edits[0].1.clone()
                }
                None => return None,
                Some(o) => {
                    let mut t = o.clone();
                    for (a, bb, all) in &edits {
                        t = apply_edit(
                            &t,
                            &a.replace("\r\n", "\n"),
                            &bb.replace("\r\n", "\n"),
                            *all,
                        )
                        .ok()?
                        .0;
                    }
                    t
                }
            }
        }
        _ => return None,
    };
    let lines = match &old {
        Some(o) => diff_lines(o, &new).0,
        None => new
            .lines()
            .enumerate()
            .map(|(i, l)| DiffLine {
                kind: '+',
                lineno: i + 1,
                text: l.into(),
            })
            .collect(),
    };
    Some((
        if old.is_some() {
            format!("Edit {dp}")
        } else {
            format!("Create {dp}")
        },
        lines,
    ))
}

// ─────────────────────────── Glob / Grep / LS ───────────────────────────

fn walker(root: &Path) -> ignore::Walk {
    ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build()
}

fn mtime(p: &Path) -> SystemTime {
    fs::metadata(p)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

fn glob(ctx: &ToolCtx, input: &Value) -> TR {
    let pattern = need(input, "pattern")?;
    let base = s(input, "path")
        .filter(|p| !p.trim().is_empty())
        .map(|p| ctx.resolve(p))
        .unwrap_or_else(|| ctx.root.clone());
    if !base.is_dir() {
        return Err(ToolOutput::err(format!(
            "Path does not exist or is not a directory: {}",
            base.display()
        )));
    }
    let pat = if pattern.contains('/') || pattern.starts_with("**") {
        pattern.to_string()
    } else {
        format!("**/{pattern}")
    };
    let g = globset::GlobBuilder::new(&pat)
        .literal_separator(true)
        .build()
        .map_err(|e| ToolOutput::err(format!("Invalid glob pattern: {e}")))?
        .compile_matcher();
    let t0 = Instant::now();
    let mut hits: Vec<PathBuf> = Vec::new();
    for e in walker(&base).flatten() {
        if ctx.cancelled() {
            break;
        }
        if !e.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        if let Ok(rel) = e.path().strip_prefix(&base) {
            if g.is_match(rel) {
                hits.push(e.path().to_path_buf());
            }
        }
    }
    hits.sort_by_key(|p| std::cmp::Reverse(mtime(p)));
    let total = hits.len();
    let truncated = total > 100;
    hits.truncate(100);
    let ms = t0.elapsed().as_millis();
    if hits.is_empty() {
        return Ok(ToolOutput::ok(
            "No files found",
            Display::Lines {
                summary: "Found 0 files".into(),
                body: vec![],
            },
        ));
    }
    let mut content: String = hits
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    if truncated {
        content
            .push_str("\n(Results are truncated. Consider using a more specific path or pattern.)");
    }
    Ok(ToolOutput::ok(
        content,
        Display::Lines {
            summary: format!("Found {total} file{} in {ms}ms", plural(total)),
            body: vec![],
        },
    ))
}

fn file_type_matcher(ty: &str) -> Option<ignore::types::Types> {
    let mut tb = ignore::types::TypesBuilder::new();
    tb.add_defaults();
    tb.select(ty);
    tb.build().ok()
}

fn grep(ctx: &ToolCtx, input: &Value) -> TR {
    let pattern = need(input, "pattern")?;
    let base = s(input, "path")
        .filter(|p| !p.trim().is_empty())
        .map(|p| ctx.resolve(p))
        .unwrap_or_else(|| ctx.root.clone());
    if !base.exists() {
        return Err(ToolOutput::err(format!(
            "Path does not exist: {}",
            base.display()
        )));
    }
    let mode = s(input, "output_mode").unwrap_or("files_with_matches");
    let multiline = b(input, "multiline");
    let re = regex::RegexBuilder::new(pattern)
        .case_insensitive(b(input, "-i"))
        .multi_line(true)
        .dot_matches_new_line(multiline)
        .build()
        .map_err(|e| ToolOutput::err(format!("Invalid regex: {e}")))?;
    let glob_m = match s(input, "glob") {
        Some(g) if !g.is_empty() => {
            let mut gb = globset::GlobSetBuilder::new();
            // "*.{ts,tsx}" and space/comma separated lists.
            for part in split_globs(g) {
                let pat = if part.contains('/') {
                    part
                } else {
                    format!("**/{part}")
                };
                if let Ok(gl) = globset::GlobBuilder::new(&pat)
                    .literal_separator(false)
                    .build()
                {
                    gb.add(gl);
                }
            }
            gb.build().ok()
        }
        _ => None,
    };
    let types = s(input, "type").and_then(file_type_matcher);
    let line_numbers = input.get("-n").map(|_| b(input, "-n")).unwrap_or(true);
    let ctx_c = n(input, "-C").unwrap_or(0) as usize;
    let after = n(input, "-A").map(|x| x as usize).unwrap_or(ctx_c);
    let before = n(input, "-B").map(|x| x as usize).unwrap_or(ctx_c);
    let head_limit = n(input, "head_limit").map(|x| x as usize);

    let files: Vec<PathBuf> = if base.is_file() {
        vec![base.clone()]
    } else {
        walker(&base)
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
            .map(|e| e.into_path())
            .filter(|p| {
                let rel = p.strip_prefix(&base).unwrap_or(p);
                glob_m.as_ref().map(|g| g.is_match(rel)).unwrap_or(true)
                    && types
                        .as_ref()
                        .map(|t| t.matched(p, false).is_whitelist())
                        .unwrap_or(true)
            })
            .collect()
    };

    let mut matched_files: Vec<(PathBuf, usize)> = Vec::new();
    let mut content_lines: Vec<String> = Vec::new();
    for f in files {
        if ctx.cancelled() {
            break;
        }
        let Ok(bytes) = fs::read(&f) else { continue };
        if looks_binary(&bytes) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        let shown = f.display().to_string();
        if multiline {
            let count = re.find_iter(&text).count();
            if count == 0 {
                continue;
            }
            matched_files.push((f.clone(), count));
            if mode == "content" {
                for m in re.find_iter(&text) {
                    let ln = text[..m.start()].matches('\n').count() + 1;
                    for (k, l) in m.as_str().split('\n').enumerate() {
                        content_lines.push(if line_numbers {
                            format!("{shown}:{}:{l}", ln + k)
                        } else {
                            format!("{shown}:{l}")
                        });
                    }
                }
            }
            continue;
        }
        let lines: Vec<&str> = text.lines().collect();
        let hits: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| re.is_match(l))
            .map(|(i, _)| i)
            .collect();
        if hits.is_empty() {
            continue;
        }
        matched_files.push((f.clone(), hits.len()));
        if mode == "content" {
            let mut last_printed: Option<usize> = None;
            for &h in &hits {
                let from = h.saturating_sub(before);
                let to = (h + after).min(lines.len() - 1);
                if let Some(lp) = last_printed {
                    if from > lp + 1 && (before > 0 || after > 0) {
                        content_lines.push("--".into());
                    }
                }
                #[allow(clippy::needless_range_loop)]
                for i in from..=to {
                    if last_printed.map(|lp| i <= lp).unwrap_or(false) {
                        continue;
                    }
                    let sep = if i == h || hits.contains(&i) {
                        ':'
                    } else {
                        '-'
                    };
                    content_lines.push(if line_numbers {
                        format!("{shown}{sep}{}{sep}{}", i + 1, lines[i])
                    } else {
                        format!("{shown}{sep}{}", lines[i])
                    });
                    last_printed = Some(i);
                }
            }
        }
    }

    match mode {
        "content" => {
            let total = content_lines.len();
            if let Some(h) = head_limit {
                content_lines.truncate(h);
            }
            let content = if content_lines.is_empty() {
                "No matches found".to_string()
            } else {
                content_lines.join("\n")
            };
            let n_hits: usize = matched_files.iter().map(|(_, c)| c).sum();
            Ok(ToolOutput::ok(
                clip_middle(&content, BASH_MAX_OUTPUT),
                Display::Lines {
                    summary: format!("Found {total} line{}", plural(total.max(n_hits.min(total)))),
                    body: vec![],
                },
            ))
        }
        "count" => {
            let mut rows: Vec<String> = matched_files
                .iter()
                .map(|(p, c)| format!("{}:{c}", p.display()))
                .collect();
            if let Some(h) = head_limit {
                rows.truncate(h);
            }
            let total: usize = matched_files.iter().map(|(_, c)| c).sum();
            let content = if rows.is_empty() {
                "No matches found".to_string()
            } else {
                format!(
                    "{}\n\nFound {total} total occurrence{} across {} file{}.",
                    rows.join("\n"),
                    plural(total),
                    matched_files.len(),
                    plural(matched_files.len())
                )
            };
            Ok(ToolOutput::ok(
                content,
                Display::Lines {
                    summary: format!("Found {total} match{}", if total == 1 { "" } else { "es" }),
                    body: vec![],
                },
            ))
        }
        _ => {
            matched_files.sort_by_key(|(p, _)| std::cmp::Reverse(mtime(p)));
            let total = matched_files.len();
            let mut rows: Vec<String> = matched_files
                .iter()
                .map(|(p, _)| p.display().to_string())
                .collect();
            if let Some(h) = head_limit {
                rows.truncate(h);
            }
            let content = if rows.is_empty() {
                "No files found".to_string()
            } else {
                format!("Found {total} file{}\n{}", plural(total), rows.join("\n"))
            };
            Ok(ToolOutput::ok(
                content,
                Display::Lines {
                    summary: format!("Found {total} file{}", plural(total)),
                    body: vec![],
                },
            ))
        }
    }
}

fn split_globs(g: &str) -> Vec<String> {
    // Keep brace groups intact while splitting on whitespace/commas outside them.
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0;
    for c in g.chars() {
        match c {
            '{' => {
                depth += 1;
                cur.push(c)
            }
            '}' => {
                depth -= 1;
                cur.push(c)
            }
            ' ' | ',' if depth == 0 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn ls(ctx: &ToolCtx, input: &Value) -> TR {
    let p = ctx.resolve(s(input, "path").unwrap_or("."));
    if !p.is_dir() {
        return Err(ToolOutput::err(format!(
            "Path does not exist or is not a directory: {}",
            p.display()
        )));
    }
    let mut ig = globset::GlobSetBuilder::new();
    if let Some(arr) = input.get("ignore").and_then(|v| v.as_array()) {
        for g in arr.iter().filter_map(|x| x.as_str()) {
            if let Ok(gl) = globset::Glob::new(g) {
                ig.add(gl);
            }
        }
    }
    let ig = ig.build().ok();
    let mut entries: Vec<(usize, PathBuf, bool)> = Vec::new();
    let mut count = 0;
    let limit = 1000;
    let mut truncated = false;
    let mut w = ignore::WalkBuilder::new(&p);
    w.hidden(true)
        .git_ignore(true)
        .require_git(false)
        .max_depth(Some(12))
        .sort_by_file_name(|a, b| a.cmp(b));
    for e in w.build().flatten() {
        if e.depth() == 0 {
            continue;
        }
        let name = e.file_name().to_string_lossy();
        if ig
            .as_ref()
            .map(|g| g.is_match(e.path()) || g.is_match(name.as_ref()))
            .unwrap_or(false)
        {
            continue;
        }
        count += 1;
        if count > limit {
            truncated = true;
            break;
        }
        entries.push((
            e.depth(),
            e.path().to_path_buf(),
            e.file_type().map(|t| t.is_dir()).unwrap_or(false),
        ));
    }
    let mut out = format!("- {}{}\n", p.display(), std::path::MAIN_SEPARATOR);
    for (depth, path, is_dir) in &entries {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.push_str(&format!(
            "{}- {}{}\n",
            "  ".repeat(*depth),
            name,
            if *is_dir { "/" } else { "" }
        ));
    }
    if truncated {
        out.push_str(&format!("\nThere are more than {limit} files in the directory. Use a more specific path, or use Glob/Grep. The first {limit} entries are included above.\n"));
    }
    let n = entries.len();
    Ok(ToolOutput::ok(
        out.trim_end().to_string(),
        Display::Lines {
            summary: format!("Listed {n} path{}", plural(n)),
            body: vec![],
        },
    ))
}

// ─────────────────────────── Bash ───────────────────────────

fn shell_program() -> (String, Vec<String>) {
    if cfg!(windows) {
        for cand in ["bash.exe", "C:\\Program Files\\Git\\bin\\bash.exe"] {
            if Command::new(cand)
                .arg("-c")
                .arg("true")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
            {
                return (cand.into(), vec!["-c".into()]);
            }
        }
        return ("cmd".into(), vec!["/C".into()]);
    }
    let sh = if Path::new("/bin/bash").exists() || which("bash") {
        "bash"
    } else {
        "sh"
    };
    (sh.into(), vec!["-c".into()])
}

pub fn which(bin: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|d| {
        let p = d.join(bin);
        p.is_file() || (cfg!(windows) && d.join(format!("{bin}.exe")).is_file())
    })
}

fn configure_child(cmd: &mut Command) {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("ICE_AGENT", "1")
        .env("GIT_PAGER", "cat")
        .env("PAGER", "cat")
        .env("GIT_EDITOR", "true")
        .env("TERM", "dumb")
        .env("NO_COLOR", "1");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
}

fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        // The shell runs in its own process group: signal the whole group so
        // grandchildren (servers, sleeps) die too and release our pipes.
        let pgid = child.id() as libc::pid_t;
        unsafe {
            libc::kill(-pgid, libc::SIGTERM);
        }
        std::thread::sleep(Duration::from_millis(100));
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn spawn_reader<R: std::io::Read + Send + 'static>(
    mut r: R,
    buf: Arc<Mutex<String>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match r.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let mut b = buf.lock().unwrap();
                    b.push_str(&String::from_utf8_lossy(&chunk[..n]));
                    // Bound memory for chatty processes.
                    if b.len() > 8 * BASH_MAX_OUTPUT {
                        let cut = b.len() - 4 * BASH_MAX_OUTPUT;
                        let cut = (cut..b.len())
                            .find(|&i| b.is_char_boundary(i))
                            .unwrap_or(b.len());
                        b.replace_range(..cut, "[… earlier output truncated …]\n");
                    }
                }
            }
        }
    })
}

/// Truncate long output, keeping the head and tail.
pub fn clip_middle(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let head_end = (0..=max * 2 / 3)
        .rev()
        .find(|&i| s.is_char_boundary(i))
        .unwrap_or(0);
    let tail_start = (s.len() - max / 3..s.len())
        .find(|&i| s.is_char_boundary(i))
        .unwrap_or(s.len());
    let removed = s[head_end..tail_start].lines().count();
    format!(
        "{}\n\n... [{removed} lines truncated] ...\n\n{}",
        &s[..head_end],
        &s[tail_start..]
    )
}

fn bash(ctx: &ToolCtx, input: &Value) -> TR {
    let command = need(input, "command")?;
    if let Err(e) = crate::sandbox::deny_command(command) {
        return Err(ToolOutput::err(e.to_string()));
    }
    let timeout = n(input, "timeout")
        .unwrap_or(BASH_DEFAULT_TIMEOUT_MS)
        .clamp(1000, BASH_MAX_TIMEOUT_MS);
    let cwd = ctx.cwd.lock().unwrap().clone();
    let cwd = if cwd.is_dir() { cwd } else { ctx.root.clone() };
    let (prog, pre) = shell_program();

    if b(input, "run_in_background") {
        let mut cmd = Command::new(&prog);
        cmd.args(&pre).arg(command).current_dir(&cwd);
        configure_child(&mut cmd);
        let mut child = cmd
            .spawn()
            .map_err(|e| ToolOutput::err(format!("failed to start shell: {e}")))?;
        let buf = Arc::new(Mutex::new(String::new()));
        if let Some(o) = child.stdout.take() {
            spawn_reader(o, buf.clone());
        }
        if let Some(e) = child.stderr.take() {
            spawn_reader(e, buf.clone());
        }
        let id = {
            let mut seq = ctx.bg_seq.lock().unwrap();
            *seq += 1;
            format!("bash_{}", *seq)
        };
        ctx.bg.lock().unwrap().insert(
            id.clone(),
            BgShell {
                command: command.to_string(),
                child,
                output: buf,
                read_upto: 0,
                started: Instant::now(),
            },
        );
        return Ok(ToolOutput::ok(
            format!("Command running in background with ID: {id}"),
            Display::Lines {
                summary: format!("Running in the background (id: {id})"),
                body: vec![],
            },
        ));
    }

    // Persist the working directory across calls: the wrapper records $PWD.
    let marker = std::env::temp_dir().join(format!(
        "ice-cwd-{}-{}",
        std::process::id(),
        crate::conv::new_id("c")
    ));
    let wrapped = if prog == "cmd" {
        command.to_string()
    } else {
        format!(
            "{command}\n__ice_ec=$?\npwd -P > '{}' 2>/dev/null\nexit $__ice_ec",
            marker.display().to_string().replace('\'', "'\\''")
        )
    };
    let mut cmd = Command::new(&prog);
    cmd.args(&pre).arg(&wrapped).current_dir(&cwd);
    configure_child(&mut cmd);
    let t0 = Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|e| ToolOutput::err(format!("failed to start shell: {e}")))?;
    let buf = Arc::new(Mutex::new(String::new()));
    let h1 = child.stdout.take().map(|o| spawn_reader(o, buf.clone()));
    let h2 = child.stderr.take().map(|e| spawn_reader(e, buf.clone()));
    let mut interrupted = false;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) => {}
            Err(_) => break None,
        }
        if ctx.cancelled() {
            interrupted = true;
            kill_tree(&mut child);
            break None;
        }
        if t0.elapsed() > Duration::from_millis(timeout) {
            timed_out = true;
            kill_tree(&mut child);
            break None;
        }
        std::thread::sleep(Duration::from_millis(15));
    };
    for h in [h1, h2].into_iter().flatten() {
        let _ = h.join();
    }
    let mut out = buf.lock().unwrap().trim_end().to_string();
    // Update the persistent cwd; reset if it wandered outside the project.
    let mut note = String::new();
    if let Ok(new_cwd) = fs::read_to_string(&marker) {
        let nc = PathBuf::from(new_cwd.trim());
        if nc.is_dir() {
            if ctx.inside_root(&nc) || std::env::var_os("ICE_ALLOW_CD_ANYWHERE").is_some() {
                *ctx.cwd.lock().unwrap() = nc;
            } else {
                *ctx.cwd.lock().unwrap() = ctx.root.clone();
                note = format!("\nShell cwd was reset to {}", ctx.root.display());
            }
        }
    }
    let _ = fs::remove_file(&marker);
    let code = status
        .and_then(|s| s.code())
        .unwrap_or(if interrupted || timed_out { 143 } else { 1 });
    if interrupted {
        out.push_str("\n[Request interrupted by user]");
    } else if timed_out {
        out.push_str(&format!("\nCommand timed out after {}", fmt_ms(timeout)));
    }
    let clipped = clip_middle(&out, BASH_MAX_OUTPUT);
    let body: Vec<String> = clipped.lines().map(String::from).collect();
    let is_error = code != 0;
    let mut content = if is_error {
        format!("Exit code {code}\n{clipped}")
    } else {
        clipped.clone()
    };
    if content.trim().is_empty() {
        content = "(Bash completed with no output)".into();
    }
    content.push_str(&note);
    Ok(ToolOutput {
        content,
        is_error,
        display: Display::Lines {
            summary: if body.is_empty() {
                if is_error {
                    format!("Error: exit code {code}")
                } else {
                    "(No content)".into()
                }
            } else if is_error {
                format!("Error: exit code {code}")
            } else {
                String::new()
            },
            body,
        },
    })
}

fn fmt_ms(ms: u64) -> String {
    if ms >= 60_000 && ms.is_multiple_of(60_000) {
        format!("{}m", ms / 60_000)
    } else if ms >= 1000 {
        format!("{}s", ms / 1000)
    } else {
        format!("{ms}ms")
    }
}

fn bash_output(ctx: &ToolCtx, input: &Value) -> TR {
    let id = s(input, "bash_id")
        .or_else(|| s(input, "shell_id"))
        .ok_or_else(|| {
            ToolOutput::err("InputValidationError: The required parameter `bash_id` is missing")
        })?;
    let mut bg = ctx.bg.lock().unwrap();
    let sh = bg
        .get_mut(id)
        .ok_or_else(|| ToolOutput::err(format!("No shell found with ID: {id}")))?;
    let status = match sh.child.try_wait() {
        Ok(Some(st)) => format!("completed (exit code {})", st.code().unwrap_or(-1)),
        _ => "running".into(),
    };
    let all = sh.output.lock().unwrap().clone();
    let start = sh.read_upto.min(all.len());
    let start = (start..=all.len())
        .find(|&i| all.is_char_boundary(i))
        .unwrap_or(all.len());
    let fresh = all[start..].to_string();
    sh.read_upto = all.len();
    let fresh = match s(input, "filter").and_then(|f| regex::Regex::new(f).ok()) {
        Some(re) => fresh
            .lines()
            .filter(|l| re.is_match(l))
            .collect::<Vec<_>>()
            .join("\n"),
        None => fresh,
    };
    let clipped = clip_middle(fresh.trim_end(), BASH_MAX_OUTPUT);
    Ok(ToolOutput::ok(
        format!(
            "<status>{status}</status>\n<command>{}</command>\n<elapsed>{}s</elapsed>\n<output>\n{clipped}\n</output>",
            sh.command,
            sh.started.elapsed().as_secs()
        ),
        Display::Lines { summary: status.clone(), body: clipped.lines().map(String::from).collect() },
    ))
}

fn kill_shell(ctx: &ToolCtx, input: &Value) -> TR {
    let id = s(input, "shell_id")
        .or_else(|| s(input, "bash_id"))
        .ok_or_else(|| {
            ToolOutput::err("InputValidationError: The required parameter `shell_id` is missing")
        })?;
    let mut bg = ctx.bg.lock().unwrap();
    match bg.remove(id) {
        Some(mut sh) => {
            kill_tree(&mut sh.child);
            Ok(ToolOutput::ok(
                format!("Successfully killed shell: {id} ({})", sh.command),
                Display::Lines {
                    summary: format!("Killed {id}"),
                    body: vec![],
                },
            ))
        }
        None => Err(ToolOutput::err(format!("No shell found with ID: {id}"))),
    }
}

/// Background shells, for /bashes and the status line.
pub fn background_shells(ctx: &ToolCtx) -> Vec<(String, String, bool)> {
    let mut bg = ctx.bg.lock().unwrap();
    let mut v: Vec<_> = bg
        .iter_mut()
        .map(|(id, s)| {
            (
                id.clone(),
                s.command.clone(),
                matches!(s.child.try_wait(), Ok(None)),
            )
        })
        .collect();
    v.sort();
    v
}

// ─────────────────────────── TodoWrite ───────────────────────────

fn todo_write(ctx: &ToolCtx, input: &Value) -> TR {
    let arr = input
        .get("todos")
        .and_then(|t| t.as_array())
        .ok_or_else(|| {
            ToolOutput::err("InputValidationError: The required parameter `todos` is missing")
        })?;
    let mut todos = Vec::new();
    for t in arr {
        let content = s(t, "content").unwrap_or("").trim().to_string();
        if content.is_empty() {
            continue;
        }
        let status = match s(t, "status").unwrap_or("pending") {
            "in_progress" | "in-progress" | "doing" => "in_progress",
            "completed" | "done" | "complete" => "completed",
            _ => "pending",
        }
        .to_string();
        let active_form = s(t, "activeForm").unwrap_or(&content).to_string();
        todos.push(TodoItem {
            content,
            status,
            active_form,
        });
    }
    *ctx.todos.lock().unwrap() = todos.clone();
    Ok(ToolOutput::ok(
        "Todos have been modified successfully. Ensure that you continue to use the todo list to track your progress. Please proceed with the current tasks if applicable",
        Display::Todos(todos),
    ))
}

// ─────────────────────────── Skill ───────────────────────────

fn skill(ctx: &ToolCtx, input: &Value) -> TR {
    let name = s(input, "skill")
        .or_else(|| s(input, "name"))
        .ok_or_else(|| {
            ToolOutput::err("InputValidationError: The required parameter `skill` is missing")
        })?;
    match crate::skills::load_named(&ctx.root, name) {
        Some(sk) => Ok(ToolOutput::ok(
            format!(
                "<skill name=\"{}\" path=\"{}\">\n{}\n</skill>",
                sk.name,
                sk.path.display(),
                sk.body
            ),
            Display::Lines {
                summary: format!("Loaded skill {}", sk.name),
                body: vec![],
            },
        )),
        None => Err(ToolOutput::err(format!("Unknown skill: {name}"))),
    }
}

// ─────────────────────────── Web ───────────────────────────

fn web_search(_ctx: &ToolCtx, input: &Value) -> TR {
    let q = need(input, "query")?;
    let allowed: Vec<String> = input
        .get("allowed_domains")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let blocked: Vec<String> = input
        .get("blocked_domains")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let t0 = Instant::now();
    let results = search_ddg(q).map_err(|e| ToolOutput::err(format!("Web search failed: {e}")))?;
    let results: Vec<_> = results
        .into_iter()
        .filter(|(_, url, _)| {
            let host = host_of(url);
            (allowed.is_empty() || allowed.iter().any(|d| host.ends_with(d.as_str())))
                && !blocked.iter().any(|d| host.ends_with(d.as_str()))
        })
        .take(10)
        .collect();
    if results.is_empty() {
        return Ok(ToolOutput::ok(
            format!("No results found for: {q}"),
            Display::Lines {
                summary: "Did 1 search · no results".into(),
                body: vec![],
            },
        ));
    }
    let mut out = format!("Web search results for query: \"{q}\"\n\n");
    for (i, (title, url, snip)) in results.iter().enumerate() {
        out.push_str(&format!("{}. [{title}]({url})\n   {snip}\n", i + 1));
    }
    Ok(ToolOutput::ok(
        out,
        Display::Lines {
            summary: format!("Did 1 search in {:.1}s", t0.elapsed().as_secs_f32()),
            body: vec![],
        },
    ))
}

pub fn host_of(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    rest.split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .rsplit('@')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn search_ddg(q: &str) -> Result<Vec<(String, String, String)>> {
    let url = format!("https://html.duckduckgo.com/html/?q={}", urlencode(q));
    let (_, html) = crate::http::get_text(
        &url,
        &[("accept", "text/html")],
        Duration::from_secs(20),
        4 << 20,
    )?;
    let link_re =
        regex::Regex::new(r#"(?s)<a[^>]*class="result__a"[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#)
            .unwrap();
    let snip_re = regex::Regex::new(r#"(?s)class="result__snippet"[^>]*>(.*?)</a>"#).unwrap();
    let snippets: Vec<String> = snip_re
        .captures_iter(&html)
        .map(|c| html_to_text(&c[1]))
        .collect();
    let mut out = Vec::new();
    for (i, c) in link_re.captures_iter(&html).enumerate() {
        let mut href = decode_entities(&c[1]);
        if let Some(pos) = href.find("uddg=") {
            let enc = href[pos + 5..].split('&').next().unwrap_or("");
            href = urldecode(enc);
        }
        if href.starts_with("//") {
            href = format!("https:{href}");
        }
        if href.contains("duckduckgo.com/y.js") {
            continue; // ads
        }
        out.push((
            html_to_text(&c[2]),
            href,
            snippets.get(i).cloned().unwrap_or_default(),
        ));
    }
    if out.is_empty() && html.contains("anomaly") {
        bail!("the search provider rate-limited this request; try again shortly");
    }
    Ok(out)
}

pub fn web_fetch_raw(ctx: &ToolCtx, url: &str) -> Result<String> {
    let mut url = url.trim().to_string();
    if url.starts_with("http://")
        && !url.starts_with("http://localhost")
        && !url.starts_with("http://127.")
    {
        url = format!("https://{}", &url[7..]);
    }
    if !url.starts_with("https://") && !url.starts_with("http://") {
        bail!("Invalid URL: {url}");
    }
    if let Some((t, body)) = ctx.fetch_cache.lock().unwrap().get(&url) {
        if t.elapsed() < Duration::from_secs(900) {
            return Ok(body.clone());
        }
    }
    let (ctype, body) = crate::http::get_text(
        &url,
        &[("accept", "text/html,text/markdown,text/plain,*/*")],
        Duration::from_secs(30),
        10 << 20,
    )?;
    let text = if ctype.contains("html") || body.trim_start().starts_with('<') {
        html_to_markdown(&body)
    } else {
        body
    };
    let text: String = text.chars().take(100_000).collect();
    ctx.fetch_cache
        .lock()
        .unwrap()
        .insert(url, (Instant::now(), text.clone()));
    Ok(text)
}

/// A readable markdown-ish rendering of an HTML page.
pub fn html_to_markdown(html: &str) -> String {
    let mut s = regex::Regex::new(r"(?is)<(script|style|noscript|svg|head|nav|footer|iframe)[^>]*>.*?</(script|style|noscript|svg|head|nav|footer|iframe)>").unwrap().replace_all(html, " ").into_owned();
    s = regex::Regex::new(r"(?s)<!--.*?-->")
        .unwrap()
        .replace_all(&s, " ")
        .into_owned();
    for (lvl, re) in (1..=6).map(|i| {
        (
            i,
            regex::Regex::new(&format!(r"(?is)<h{i}[^>]*>(.*?)</h{i}>")).unwrap(),
        )
    }) {
        s = re
            .replace_all(&s, |c: &regex::Captures| {
                format!("\n\n{} {}\n\n", "#".repeat(lvl), html_to_text(&c[1]))
            })
            .into_owned();
    }
    s = regex::Regex::new(r#"(?is)<a[^>]*href="([^"]*)"[^>]*>(.*?)</a>"#)
        .unwrap()
        .replace_all(&s, |c: &regex::Captures| {
            let t = html_to_text(&c[2]);
            if t.is_empty() {
                String::new()
            } else {
                format!("[{t}]({})", &c[1])
            }
        })
        .into_owned();
    s = regex::Regex::new(r"(?is)<li[^>]*>")
        .unwrap()
        .replace_all(&s, "\n- ")
        .into_owned();
    s = regex::Regex::new(r"(?is)<(pre|code)[^>]*>")
        .unwrap()
        .replace_all(&s, "`")
        .into_owned();
    s = regex::Regex::new(r"(?is)</(pre|code)>")
        .unwrap()
        .replace_all(&s, "`")
        .into_owned();
    s = regex::Regex::new(r"(?is)<(br|/p|/div|/tr|/section|/article|/ul|/ol|/table)[^>]*>")
        .unwrap()
        .replace_all(&s, "\n")
        .into_owned();
    s = regex::Regex::new(r"(?s)<[^>]+>")
        .unwrap()
        .replace_all(&s, "")
        .into_owned();
    let s = decode_entities(&s);
    let mut out = String::new();
    let mut blank = 0;
    for line in s.lines() {
        let l = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if l.is_empty() {
            blank += 1;
            if blank == 1 && !out.is_empty() {
                out.push('\n');
            }
        } else {
            blank = 0;
            out.push_str(&l);
            out.push('\n');
        }
    }
    out.trim().to_string()
}

pub fn html_to_text(s: &str) -> String {
    let t = regex::Regex::new(r"(?s)<[^>]+>")
        .unwrap()
        .replace_all(s, "");
    decode_entities(&t)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn decode_entities(s: &str) -> String {
    let s = s
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&mdash;", "—")
        .replace("&ndash;", "–")
        .replace("&hellip;", "…");
    let re = regex::Regex::new(r"&#(x?)([0-9a-fA-F]+);").unwrap();
    let s = re.replace_all(&s, |c: &regex::Captures| {
        let n = if &c[1] == "x" {
            u32::from_str_radix(&c[2], 16).ok()
        } else {
            c[2].parse().ok()
        };
        n.and_then(char::from_u32)
            .map(|ch| ch.to_string())
            .unwrap_or_default()
    });
    s.replace("&amp;", "&")
}

pub fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
                out.push(b'%');
            }
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Validate required keys from a JSON schema before running a tool, giving
/// the model the same InputValidationError shape Claude Code does.
pub fn validate(spec: &ToolSpec, input: &Value) -> Result<()> {
    if !input.is_object() {
        return Err(anyhow!(
            "InputValidationError: tool input must be a JSON object"
        ));
    }
    if let Some(raw) = input.get("_raw") {
        return Err(anyhow!(
            "InputValidationError: tool input was not valid JSON: {}",
            raw.as_str().unwrap_or("")
        ));
    }
    if let Some(req) = spec.input_schema.get("required").and_then(|r| r.as_array()) {
        let missing: Vec<&str> = req
            .iter()
            .filter_map(|k| k.as_str())
            .filter(|k| input.get(*k).map(|v| v.is_null()).unwrap_or(true))
            .collect();
        if !missing.is_empty() {
            return Err(anyhow!(
                "InputValidationError: {} failed due to the following issue{}:\n{}",
                spec.name,
                if missing.len() == 1 { "" } else { "s" },
                missing
                    .iter()
                    .map(|k| format!("The required parameter `{k}` is missing"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> (PathBuf, ToolCtx) {
        let d = std::env::temp_dir().join(format!("ice-tools-{}", crate::conv::new_id("t")));
        fs::create_dir_all(&d).unwrap();
        let ctx = ToolCtx::new(d.clone(), Arc::new(AtomicBool::new(false)));
        (d, ctx)
    }

    #[test]
    fn read_is_cat_n_and_enables_edit() {
        let (d, ctx) = tmp();
        fs::write(d.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        let e = run(
            &ctx,
            "Edit",
            &json!({"file_path":"a.txt","old_string":"two","new_string":"2"}),
        );
        assert!(e.is_error);
        assert!(e.content.contains("has not been read yet"));
        let r = run(&ctx, "Read", &json!({"file_path":"a.txt"}));
        assert!(!r.is_error);
        assert_eq!(r.content, "     1\tone\n     2\ttwo\n     3\tthree");
        let e = run(
            &ctx,
            "Edit",
            &json!({"file_path":"a.txt","old_string":"two","new_string":"2"}),
        );
        assert!(!e.is_error, "{}", e.content);
        assert_eq!(
            fs::read_to_string(d.join("a.txt")).unwrap(),
            "one\n2\nthree\n"
        );
        match e.display {
            Display::Diff { summary, .. } => {
                assert_eq!(summary, "Updated a.txt with 1 addition and 1 removal")
            }
            _ => panic!(),
        }
    }

    #[test]
    fn edit_errors_match_claude_code() {
        let (d, ctx) = tmp();
        fs::write(d.join("b.txt"), "x x\n").unwrap();
        run(&ctx, "Read", &json!({"file_path":"b.txt"}));
        let e = run(
            &ctx,
            "Edit",
            &json!({"file_path":"b.txt","old_string":"x","new_string":"y"}),
        );
        assert!(e
            .content
            .starts_with("Found 2 matches of the string to replace, but replace_all is false."));
        let e = run(
            &ctx,
            "Edit",
            &json!({"file_path":"b.txt","old_string":"zz","new_string":"y"}),
        );
        assert!(e
            .content
            .starts_with("String to replace not found in file."));
        let e = run(
            &ctx,
            "Edit",
            &json!({"file_path":"b.txt","old_string":"x","new_string":"x"}),
        );
        assert!(e.content.starts_with("No changes to make"));
        let e = run(
            &ctx,
            "Edit",
            &json!({"file_path":"b.txt","old_string":"x","new_string":"y","replace_all":true}),
        );
        assert!(!e.is_error);
        assert_eq!(fs::read_to_string(d.join("b.txt")).unwrap(), "y y\n");
    }

    #[test]
    fn edit_preserves_crlf_and_detects_external_changes() {
        let (d, ctx) = tmp();
        fs::write(d.join("w.txt"), "a\r\nb\r\n").unwrap();
        run(&ctx, "Read", &json!({"file_path":"w.txt"}));
        let e = run(
            &ctx,
            "Edit",
            &json!({"file_path":"w.txt","old_string":"a\nb","new_string":"a\nc"}),
        );
        assert!(!e.is_error, "{}", e.content);
        assert_eq!(fs::read_to_string(d.join("w.txt")).unwrap(), "a\r\nc\r\n");
        // Simulate a linter touching the file afterwards.
        std::thread::sleep(Duration::from_millis(20));
        let f = fs::OpenOptions::new()
            .write(true)
            .open(d.join("w.txt"))
            .unwrap();
        f.set_modified(SystemTime::now() + Duration::from_secs(5))
            .unwrap();
        let e = run(&ctx, "Write", &json!({"file_path":"w.txt","content":"z"}));
        assert!(e.content.contains("modified since read"));
    }

    #[test]
    fn write_creates_new_files_without_read() {
        let (d, ctx) = tmp();
        let w = run(
            &ctx,
            "Write",
            &json!({"file_path":"sub/new.rs","content":"fn main() {}\n"}),
        );
        assert!(!w.is_error, "{}", w.content);
        assert!(d.join("sub/new.rs").exists());
    }

    #[test]
    fn multiedit_is_atomic() {
        let (d, ctx) = tmp();
        fs::write(d.join("m.txt"), "alpha beta\n").unwrap();
        run(&ctx, "Read", &json!({"file_path":"m.txt"}));
        let e = run(
            &ctx,
            "MultiEdit",
            &json!({"file_path":"m.txt","edits":[
                {"old_string":"alpha","new_string":"A"},
                {"old_string":"missing","new_string":"B"}
            ]}),
        );
        assert!(e.is_error);
        assert!(e.content.starts_with("Edit 2 failed"));
        assert_eq!(fs::read_to_string(d.join("m.txt")).unwrap(), "alpha beta\n");
    }

    #[test]
    fn glob_grep_ls_find_files() {
        let (d, ctx) = tmp();
        fs::create_dir_all(d.join("src/deep")).unwrap();
        fs::write(
            d.join("src/main.rs"),
            "fn main() {\n    println!(\"hi\");\n}\n",
        )
        .unwrap();
        fs::write(d.join("src/deep/lib.rs"), "pub fn hi() {}\n").unwrap();
        fs::write(d.join("README.md"), "# hi\n").unwrap();
        let g = run(&ctx, "Glob", &json!({"pattern":"**/*.rs"}));
        assert_eq!(g.content.lines().count(), 2, "{}", g.content);
        let g = run(&ctx, "Glob", &json!({"pattern":"*.md"}));
        assert!(g.content.ends_with("README.md"));
        let gr = run(&ctx, "Grep", &json!({"pattern":"fn \\w+","type":"rust"}));
        assert!(gr.content.starts_with("Found 2 files"), "{}", gr.content);
        let gr = run(
            &ctx,
            "Grep",
            &json!({"pattern":"println","output_mode":"content","-C":1,"glob":"*.rs"}),
        );
        assert!(
            gr.content.contains("main.rs:2:    println!"),
            "{}",
            gr.content
        );
        assert!(
            gr.content.contains("main.rs-1-fn main() {"),
            "{}",
            gr.content
        );
        let c = run(&ctx, "Grep", &json!({"pattern":"hi","output_mode":"count"}));
        assert!(c.content.contains("total occurrence"));
        let l = run(&ctx, "LS", &json!({"path":"."}));
        assert!(l.content.contains("  - src/"), "{}", l.content);
        assert!(l.content.contains("    - main.rs"), "{}", l.content);
    }

    #[cfg(unix)]
    #[test]
    fn bash_persists_cwd_reports_exit_codes_and_times_out() {
        let (d, ctx) = tmp();
        fs::create_dir_all(d.join("sub")).unwrap();
        let r = run(&ctx, "Bash", &json!({"command":"cd sub && pwd"}));
        assert!(!r.is_error, "{}", r.content);
        let r = run(&ctx, "Bash", &json!({"command":"pwd"}));
        assert!(r.content.trim().ends_with("/sub"), "{}", r.content);
        let r = run(&ctx, "Bash", &json!({"command":"cd / && pwd"}));
        assert!(r.content.contains("Shell cwd was reset"), "{}", r.content);
        let r = run(&ctx, "Bash", &json!({"command":"echo oops >&2; exit 3"}));
        assert!(r.is_error);
        assert!(r.content.starts_with("Exit code 3\noops"), "{}", r.content);
        let t0 = Instant::now();
        let r = run(&ctx, "Bash", &json!({"command":"sleep 30","timeout":1000}));
        assert!(t0.elapsed() < Duration::from_secs(5));
        assert!(r.content.contains("timed out"), "{}", r.content);
    }

    #[cfg(unix)]
    #[test]
    fn background_shells_stream_output() {
        let (_d, ctx) = tmp();
        let r = run(
            &ctx,
            "Bash",
            &json!({"command":"echo start; sleep 0.2; echo end","run_in_background":true}),
        );
        assert!(r.content.contains("bash_1"));
        std::thread::sleep(Duration::from_millis(600));
        let o = run(&ctx, "BashOutput", &json!({"bash_id":"bash_1"}));
        assert!(
            o.content.contains("start") && o.content.contains("end"),
            "{}",
            o.content
        );
        assert!(o.content.contains("completed"));
        let o = run(&ctx, "BashOutput", &json!({"bash_id":"bash_1"}));
        let output = o.content.split("<output>").nth(1).unwrap_or("");
        assert!(!output.contains("start"), "{}", o.content);
    }

    #[test]
    fn todos_and_validation() {
        let (_d, ctx) = tmp();
        let t = run(
            &ctx,
            "TodoWrite",
            &json!({"todos":[{"content":"Run tests","status":"in_progress","activeForm":"Running tests"}]}),
        );
        assert!(!t.is_error);
        assert_eq!(ctx.todos.lock().unwrap()[0].active_form, "Running tests");
        let specs = builtin_specs(&[]);
        let read = specs.iter().find(|s| s.name == "Read").unwrap();
        let err = validate(read, &json!({})).unwrap_err().to_string();
        assert!(err.contains("The required parameter `file_path` is missing"));
    }

    #[test]
    fn html_and_urls() {
        let md = html_to_markdown("<html><head><title>x</title></head><body><h1>Title</h1><p>Hello <a href=\"https://a.b\">link</a> &amp; more</p><script>bad()</script></body></html>");
        assert!(md.contains("# Title"));
        assert!(md.contains("[link](https://a.b)"));
        assert!(md.contains("& more"));
        assert!(!md.contains("bad()"));
        assert_eq!(host_of("https://user@docs.rs:443/x?y"), "docs.rs");
        assert_eq!(urldecode("https%3A%2F%2Fa.b%2Fc"), "https://a.b/c");
        assert_eq!(canonical_name("str_replace").as_deref(), Some("Edit"));
        assert_eq!(clip_middle("abc", 10), "abc");
        assert!(clip_middle(&"x\n".repeat(50_000), 1000).contains("lines truncated"));
    }
}
