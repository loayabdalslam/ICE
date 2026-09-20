use crate::ir::Action;
use crate::sandbox::{clip, deny_command, resolve};
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct StepResult {
    pub title: String,
    pub ok: bool,
    pub exit: i32,
    pub output: String,
    pub ms: u128,
}

pub fn run_action(root: &Path, action: &Action) -> StepResult {
    let t0 = Instant::now();
    let (title, res) = match action {
        Action::Read { path } => (format!("read {path}"), read_file(root, path)),
        Action::List { path } => (format!("list {path}"), list_dir(root, path)),
        Action::Grep { pattern, path } => {
            (format!("grep {pattern} {path}"), grep(root, pattern, path))
        }
        Action::Write { path, contents } => {
            (format!("write {path}"), write_file(root, path, contents))
        }
        Action::Replace { path, old, new } => {
            (format!("replace {path}"), replace_in(root, path, old, new))
        }
        Action::Run { cmd } => (format!("run {cmd}"), run_shell(root, cmd)),
        Action::WebSearch { query } => (format!("web_search {query}"), web_search(query)),
        Action::WebFetch { url } => (format!("web_fetch {url}"), web_fetch(url)),
        Action::Yield { reason } => (
            format!("yield {reason}"),
            Ok((0, format!("YIELD: {reason}"))),
        ),
        Action::Agent { name, goal } => (
            format!("agent {name}"),
            Ok((0, format!("DELEGATE {name}: {goal}"))),
        ),
        Action::Skill { name } => (format!("skill {name}"), apply_skill(root, name)),
        Action::Mcp { tool, args } => (format!("mcp {tool}"), call_mcp(root, tool, args)),
        Action::TodoAdd { text } => (format!("todo + {text}"), add_todo(root, text)),
        Action::TodoDone { key } => (format!("todo ✓ {key}"), done_todo(root, key)),
    };
    match res {
        Ok((exit, output)) => StepResult {
            title,
            ok: exit == 0,
            exit,
            output: clip(&output, 8_000),
            ms: t0.elapsed().as_millis(),
        },
        Err(e) => StepResult {
            title,
            ok: false,
            exit: 1,
            output: e.to_string(),
            ms: t0.elapsed().as_millis(),
        },
    }
}

fn apply_skill(root: &Path, name: &str) -> Result<(i32, String)> {
    match crate::skills::load_named(root, name) {
        Some(s) => Ok((
            0,
            format!("loaded skill {} ({} bytes)", s.name, s.body.len()),
        )),
        None => bail!("unknown skill {name}"),
    }
}

fn call_mcp(root: &Path, tool: &str, args: &str) -> Result<(i32, String)> {
    crate::mcp::call_tool(root, tool, args).map(|s| (0, s))
}

fn add_todo(root: &Path, text: &str) -> Result<(i32, String)> {
    let t = crate::todos::add(root, text);
    Ok((0, format!("todo [{}] {}", t.id, t.text)))
}

fn done_todo(root: &Path, key: &str) -> Result<(i32, String)> {
    match crate::todos::complete(root, key) {
        Some(t) => Ok((0, format!("done [{}] {}", t.id, t.text))),
        None => bail!("todo not found: {key}"),
    }
}

fn read_file(root: &Path, path: &str) -> Result<(i32, String)> {
    let p = resolve(root, path)?;
    let data = fs::read_to_string(&p)?;
    Ok((0, numbered(&data)))
}

fn numbered(s: &str) -> String {
    s.lines()
        .enumerate()
        .map(|(i, l)| format!("{:>4}│ {l}", i + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

fn list_dir(root: &Path, path: &str) -> Result<(i32, String)> {
    let p = resolve(root, path)?;
    let mut names: Vec<String> = fs::read_dir(&p)?
        .filter_map(|e| e.ok())
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() {
                format!("{name}/")
            } else {
                name
            }
        })
        .collect();
    names.sort();
    Ok((0, names.join("\n")))
}

fn grep(root: &Path, pattern: &str, path: &str) -> Result<(i32, String)> {
    let p = resolve(root, path)?;
    let re = regex::Regex::new(pattern).unwrap_or_else(|_| regex::Regex::new(".*").unwrap());
    let mut hits = Vec::new();
    walk(&p, &mut |file| {
        if let Ok(text) = fs::read_to_string(file) {
            for (i, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    hits.push(format!("{}:{}:{line}", file.display(), i + 1));
                    if hits.len() >= 80 {
                        return;
                    }
                }
            }
        }
    });
    let code = if hits.is_empty() { 1 } else { 0 };
    Ok((code, hits.join("\n")))
}

fn walk(path: &Path, f: &mut impl FnMut(&PathBuf)) {
    if path.is_file() {
        f(&path.to_path_buf());
        return;
    }
    let Ok(rd) = fs::read_dir(path) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        if p.is_dir() {
            walk(&p, f);
        } else {
            f(&p);
        }
    }
}

fn write_file(root: &Path, path: &str, contents: &str) -> Result<(i32, String)> {
    let p = resolve(root, path)?;
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&p, contents)?;
    Ok((
        0,
        format!("wrote {} bytes to {}", contents.len(), p.display()),
    ))
}

fn replace_in(root: &Path, path: &str, old: &str, new: &str) -> Result<(i32, String)> {
    let p = resolve(root, path)?;
    let text = fs::read_to_string(&p)?;
    if !text.contains(old) {
        bail!("pattern not found in {path}");
    }
    let updated = text.replacen(old, new, 1);
    fs::write(&p, &updated)?;
    Ok((0, format!("replaced in {}", p.display())))
}

/// Web search via DuckDuckGo's key-free JSON API. Returns an instant-answer
/// abstract plus top related results (title + URL), suitable for agentic loops.
fn web_search(query: &str) -> Result<(i32, String)> {
    let enc = urlencode(query);
    let url = format!("https://api.duckduckgo.com/?q={enc}&format=json&no_html=1&no_redirect=1&t=ice");
    let out = Command::new("curl")
        .args(["-sSL", "-A", "ice-agent/0.3", "--max-time", "20", &url])
        .output()
        .context("curl required for web search")?;
    if !out.status.success() {
        bail!("web search failed");
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or(serde_json::Value::Null);
    let mut buf = String::new();
    if let Some(a) = v.get("AbstractText").and_then(|x| x.as_str()) {
        if !a.is_empty() {
            let src = v.get("AbstractURL").and_then(|x| x.as_str()).unwrap_or("");
            buf.push_str(&format!("{a}\n{src}\n\n"));
        }
    }
    if let Some(ans) = v.get("Answer").and_then(|x| x.as_str()) {
        if !ans.is_empty() {
            buf.push_str(&format!("Answer: {ans}\n\n"));
        }
    }
    let mut n = 0;
    if let Some(topics) = v.get("RelatedTopics").and_then(|x| x.as_array()) {
        for t in topics {
            // Skip nested category groups.
            let text = t.get("Text").and_then(|x| x.as_str());
            let first = t.get("FirstURL").and_then(|x| x.as_str());
            if let (Some(text), Some(first)) = (text, first) {
                buf.push_str(&format!("{}. {text}\n   {first}\n", n + 1));
                n += 1;
                if n >= 8 {
                    break;
                }
            }
        }
    }
    if buf.trim().is_empty() {
        return Ok((1, format!("no results for: {query}")));
    }
    Ok((0, buf))
}

/// Fetch a URL and return readable text (tags stripped, clipped).
fn web_fetch(url: &str) -> Result<(i32, String)> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        bail!("web_fetch needs an http(s) URL");
    }
    let out = Command::new("curl")
        .args(["-sSL", "-A", "ice-agent/0.3", "--max-time", "25", url])
        .output()
        .context("curl required for web fetch")?;
    if !out.status.success() {
        bail!("fetch failed: {url}");
    }
    let html = String::from_utf8_lossy(&out.stdout);
    Ok((0, strip_html(&html)))
}

fn strip_html(html: &str) -> String {
    // Drop script/style blocks, then tags, then collapse whitespace.
    let re_block = regex::Regex::new(r"(?is)<(script|style)[^>]*>.*?</(script|style)>").unwrap();
    let no_block = re_block.replace_all(html, " ");
    let re_tag = regex::Regex::new(r"(?s)<[^>]+>").unwrap();
    let text = re_tag.replace_all(&no_block, " ");
    let text = text
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"");
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(4000).collect()
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn run_shell(root: &Path, cmd: &str) -> Result<(i32, String)> {
    deny_command(cmd)?;
    let output = Command::new("bash")
        .arg("-lc")
        .arg(cmd)
        .current_dir(root)
        .output()?;
    let mut buf = String::new();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stdout.is_empty() {
        buf.push_str(&stdout);
    }
    if !stderr.is_empty() {
        if !buf.is_empty() {
            buf.push('\n');
        }
        buf.push_str(&stderr);
    }
    let code = output.status.code().unwrap_or(1);
    Ok((code, buf))
}
