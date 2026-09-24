use anyhow::{bail, Result};

/// A tool call parsed from a model that has no native tool calling (the
/// text tool protocol): XML-ish `<function=…>` markup, JSON tool_calls, or
/// ICE's plain action lines.
#[derive(Debug, Clone)]
pub enum Action {
    Read {
        path: String,
    },
    Grep {
        pattern: String,
        path: String,
    },
    List {
        path: String,
    },
    Write {
        path: String,
        contents: String,
    },
    Replace {
        path: String,
        old: String,
        new: String,
    },
    Run {
        cmd: String,
    },
    WebSearch {
        query: String,
    },
    WebFetch {
        url: String,
    },
    /// Child ICE loop. Name is a short handle; goal is the delegated task.
    Agent {
        name: String,
        goal: String,
    },
    Skill {
        name: String,
    },
    Mcp {
        tool: String,
        args: String,
    },
}

/// One step of the agentic loop: any leading narration text plus the tool
/// actions to run. No actions ⇒ this is the model's final answer.
#[derive(Debug, Clone)]
pub struct AgentStep {
    pub text: String,
    pub actions: Vec<Action>,
}

/// Parse a model turn into narration + actions, understanding both native
/// tool-call markup and ICE's plain action lines. Prose with no tool line is
/// treated as the final answer (empty actions).
pub fn parse_agent_step(raw: &str) -> AgentStep {
    let text = strip_fences(raw);
    if text.contains("<function=") || text.contains("<tool_call") || text.contains("\"tool_calls\"")
    {
        let actions = extract_tool_calls(&text);
        if !actions.is_empty() {
            return AgentStep {
                text: strip_tool_markup(&text),
                actions,
            };
        }
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut actions = Vec::new();
    let mut prose = String::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        if line.is_empty() {
            i += 1;
            continue;
        }
        let (verb, _) = split_verb(line);
        if is_action_verb(&verb) {
            if let Ok((a, next)) = parse_action(&lines, i) {
                actions.push(a);
                i = next;
                continue;
            }
        }
        if actions.is_empty() {
            if !prose.is_empty() {
                prose.push('\n');
            }
            prose.push_str(lines[i]);
        }
        i += 1;
    }
    AgentStep {
        text: prose,
        actions,
    }
}

/// Verbs that unambiguously start a tool line in freeform text. Deliberately
/// excludes bare words like "done"/"web"/"fetch" that collide with prose.
fn is_action_verb(v: &str) -> bool {
    matches!(
        v,
        "read"
            | "list"
            | "ls"
            | "grep"
            | "run"
            | "exec"
            | "sh"
            | "bash"
            | "write"
            | "replace"
            | "patch"
            | "skill"
            | "mcp"
            | "web_search"
            | "websearch"
            | "web_fetch"
            | "webfetch"
            | "agent"
            | "spawn"
            | "subagent"
            | "todo"
            | "todo_add"
            | "todo_done"
    )
}

fn strip_tool_markup(text: &str) -> String {
    let mut s = text.to_string();
    for pat in [
        r"(?s)<tool_call>.*?</tool_call>",
        r"(?s)<function=.*?</function>",
    ] {
        if let Ok(re) = regex::Regex::new(pat) {
            s = re.replace_all(&s, "").to_string();
        }
    }
    s.trim().to_string()
}

/// Extract ICE actions from model-native tool-call markup: XML-ish
/// `<function=NAME><parameter=key>value</parameter></function>` blocks and,
/// as a fallback, `{"name":..,"arguments":{..}}` JSON objects.
fn extract_tool_calls(text: &str) -> Vec<Action> {
    let mut actions = Vec::new();
    let fn_re = regex::Regex::new(r"(?s)<function=([A-Za-z_]+)\s*>(.*?)</function>").ok();
    let param_re = regex::Regex::new(r"(?s)<parameter=([A-Za-z_]+)\s*>(.*?)</parameter>").ok();
    if let (Some(fn_re), Some(param_re)) = (fn_re, param_re) {
        for cap in fn_re.captures_iter(text) {
            let name = cap[1].to_ascii_lowercase();
            let body = &cap[2];
            let mut params: Vec<(String, String)> = Vec::new();
            for p in param_re.captures_iter(body) {
                params.push((p[1].to_ascii_lowercase(), p[2].trim().to_string()));
            }
            if let Some(a) = action_from_call(&name, &params) {
                actions.push(a);
            }
        }
    }
    if actions.is_empty() {
        // JSON tool-call objects: {"name":"read","arguments":{"file_path":"x"}}
        if let Ok(re) = regex::Regex::new(r#"(?s)\{[^{}]*"name"\s*:\s*"([A-Za-z_]+)"[^{}]*\}"#) {
            for cap in re.captures_iter(text) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&cap[0]) {
                    let name = v
                        .get("name")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    let args = v.get("arguments").or_else(|| v.get("parameters"));
                    let mut params = Vec::new();
                    if let Some(obj) = args.and_then(|a| a.as_object()) {
                        for (k, val) in obj {
                            let s = val
                                .as_str()
                                .map(String::from)
                                .unwrap_or_else(|| val.to_string());
                            params.push((k.to_ascii_lowercase(), s));
                        }
                    }
                    if let Some(a) = action_from_call(&name, &params) {
                        actions.push(a);
                    }
                }
            }
        }
    }
    actions
}

fn pick(params: &[(String, String)], keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| {
        params
            .iter()
            .find(|(pk, _)| pk == k)
            .map(|(_, v)| v.clone())
    })
}

fn action_from_call(name: &str, params: &[(String, String)]) -> Option<Action> {
    match name {
        "read" | "cat" | "view" | "open" | "read_file" => Some(Action::Read {
            path: pick(params, &["file_path", "path", "filename", "file"])?,
        }),
        "list" | "ls" | "list_dir" | "listdir" => Some(Action::List {
            path: pick(params, &["path", "dir", "directory"]).unwrap_or_else(|| ".".into()),
        }),
        "grep" | "search" | "find" => Some(Action::Grep {
            pattern: pick(params, &["pattern", "query", "regex", "text"]).unwrap_or_default(),
            path: pick(params, &["path", "dir", "directory"]).unwrap_or_else(|| ".".into()),
        }),
        "write" | "create" | "create_file" | "write_file" | "new_file" => Some(Action::Write {
            path: pick(params, &["file_path", "path", "filename", "file"])?,
            contents: pick(params, &["content", "contents", "text", "body", "data"])
                .unwrap_or_default(),
        }),
        "replace" | "edit" | "str_replace" | "patch" | "edit_file" => Some(Action::Replace {
            path: pick(params, &["file_path", "path", "filename", "file"])?,
            old: pick(params, &["old", "old_str", "old_string", "search"]).unwrap_or_default(),
            new: pick(
                params,
                &["new", "new_str", "new_string", "replace", "content"],
            )
            .unwrap_or_default(),
        }),
        "run" | "bash" | "shell" | "exec" | "execute" | "command" | "run_command" => {
            Some(Action::Run {
                cmd: pick(params, &["command", "cmd", "script", "code"])?,
            })
        }
        "web_search" | "websearch" | "web" | "google" | "ddg" | "search_web" | "browse" => {
            Some(Action::WebSearch {
                query: pick(params, &["query", "q", "text", "search", "term"])?,
            })
        }
        "web_fetch" | "webfetch" | "fetch" | "open_url" | "fetch_url" | "get_url" => {
            Some(Action::WebFetch {
                url: pick(params, &["url", "link", "href", "uri"])?,
            })
        }
        "mcp" | "call_tool" | "use_tool" | "tool" | "mcp_call" => Some(Action::Mcp {
            tool: pick(params, &["tool", "name", "server_tool", "qualified"]).unwrap_or_default(),
            args: pick(params, &["args", "arguments", "input", "params"])
                .unwrap_or_else(|| params_to_json(params)),
        }),
        "skill" | "use_skill" | "load_skill" | "apply_skill" => Some(Action::Skill {
            name: pick(params, &["name", "skill", "id"])?,
        }),
        "agent" | "spawn" | "subagent" | "delegate" | "task" => Some(Action::Agent {
            name: slug(
                &pick(params, &["name", "type", "agent", "role"])
                    .unwrap_or_else(|| "worker".into()),
            ),
            goal: pick(
                params,
                &["goal", "task", "prompt", "instructions", "description"],
            )
            .unwrap_or_default(),
        }),
        "yield" | "ask" | "ask_user" | "need_input" | "finish" | "done" | "todo" | "add_todo"
        | "todo_add" => None,
        _ => {
            // Unknown function: route it to MCP so ICE can drive any tool a
            // model invents, in any format. Qualified names (server/tool or
            // server__tool) pass through; bare names hit the first server.
            if name.is_empty() {
                None
            } else {
                let tool = name.replace("__", "/");
                Some(Action::Mcp {
                    tool,
                    args: params_to_json(params),
                })
            }
        }
    }
}

fn params_to_json(params: &[(String, String)]) -> String {
    let map: serde_json::Map<String, serde_json::Value> = params
        .iter()
        .map(|(k, v)| {
            // Keep JSON values as-is when the param already holds JSON.
            let val = serde_json::from_str::<serde_json::Value>(v)
                .unwrap_or_else(|_| serde_json::Value::String(v.clone()));
            (k.clone(), val)
        })
        .collect();
    serde_json::Value::Object(map).to_string()
}

fn strip_fences(s: &str) -> String {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("```") {
        let rest = rest.find('\n').map(|i| &rest[i + 1..]).unwrap_or(rest);
        return rest
            .rsplit_once("```")
            .map(|(a, _)| a.to_string())
            .unwrap_or_else(|| rest.to_string());
    }
    t.to_string()
}

fn parse_action(lines: &[&str], i: usize) -> Result<(Action, usize)> {
    let line = lines[i].trim();
    let (verb, rest) = split_verb(line);

    match verb.as_str() {
        "read" => Ok((
            Action::Read {
                path: need_arg(&rest, "read")?,
            },
            i + 1,
        )),
        "list" | "ls" => Ok((
            Action::List {
                path: if rest.is_empty() { ".".into() } else { rest },
            },
            i + 1,
        )),
        "grep" => {
            let (pattern, path) = split_grep(&rest);
            Ok((Action::Grep { pattern, path }, i + 1))
        }
        "run" | "exec" | "sh" | "bash" => Ok((
            Action::Run {
                cmd: need_arg(&rest, "run")?,
            },
            i + 1,
        )),
        "web_search" | "websearch" | "web" | "google" | "ddg" => Ok((
            Action::WebSearch {
                query: need_arg(&rest, "web_search")?,
            },
            i + 1,
        )),
        "web_fetch" | "webfetch" | "fetch" | "open_url" | "curl_url" => Ok((
            Action::WebFetch {
                url: need_arg(&rest, "web_fetch")?,
            },
            i + 1,
        )),
        "agent" | "spawn" | "subagent" => Ok((parse_agent(&rest), i + 1)),
        "skill" => Ok((
            Action::Skill {
                name: need_arg(&rest, "skill")?,
            },
            i + 1,
        )),
        "mcp" | "tool" => {
            let (tool, args) = match rest.split_once(char::is_whitespace) {
                Some((t, a)) => (t.to_string(), a.trim().to_string()),
                None => (rest, "{}".into()),
            };
            let args = if args.is_empty() { "{}".into() } else { args };
            Ok((Action::Mcp { tool, args }, i + 1))
        }
        "write" => {
            let path = need_arg(&rest, "write")?;
            let (body, next) = take_block(lines, i + 1, &[">>", "EOF", "```"])?;
            Ok((
                Action::Write {
                    path,
                    contents: body,
                },
                next,
            ))
        }
        "replace" | "patch" => {
            let path = need_arg(&rest, "replace")?;
            let (old, new, next) = take_replace(lines, i + 1)?;
            Ok((Action::Replace { path, old, new }, next))
        }
        other => {
            // Allow "run-less" shell lines inside BURST
            if looks_like_shell(line) {
                Ok((
                    Action::Run {
                        cmd: line.to_string(),
                    },
                    i + 1,
                ))
            } else {
                bail!("unknown action '{other}'");
            }
        }
    }
}

fn split_verb(line: &str) -> (String, String) {
    let line = line.trim_start_matches(['-', '*', '•']).trim();
    match line.split_once(char::is_whitespace) {
        Some((v, r)) => (v.to_ascii_lowercase(), r.trim().to_string()),
        None => (line.to_ascii_lowercase(), String::new()),
    }
}

fn need_arg(rest: &str, verb: &str) -> Result<String> {
    if rest.is_empty() {
        bail!("{verb} requires a path or argument");
    }
    Ok(unquote(rest))
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

fn split_grep(rest: &str) -> (String, String) {
    let rest = rest.trim();
    if rest.is_empty() {
        return (String::new(), ".".into());
    }
    if let Some(quoted) = rest.strip_prefix('"') {
        if let Some(end) = quoted.find('"') {
            let pat = quoted[..end].to_string();
            let path = quoted[end + 1..].trim();
            return (
                pat,
                if path.is_empty() {
                    ".".into()
                } else {
                    unquote(path)
                },
            );
        }
    }
    match rest.split_once(char::is_whitespace) {
        Some((p, path)) => (unquote(p), unquote(path)),
        None => (unquote(rest), ".".into()),
    }
}

fn take_block(lines: &[&str], start: usize, closers: &[&str]) -> Result<(String, usize)> {
    let mut i = start;
    // skip opener << or <<'EOF' or ```
    if i < lines.len() {
        let t = lines[i].trim();
        if t.starts_with("<<") || t == "<<" || t.starts_with("```") || t == "{" {
            i += 1;
        }
    }
    let mut body = String::new();
    while i < lines.len() {
        let t = lines[i].trim();
        if closers.iter().any(|c| t == *c || t.starts_with(c)) {
            return Ok((body, i + 1));
        }
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(lines[i]);
        i += 1;
    }
    Ok((body, i))
}

fn take_replace(lines: &[&str], start: usize) -> Result<(String, String, usize)> {
    let mut i = start;
    // skip <<<<
    if i < lines.len() && lines[i].trim().starts_with("<<<<") {
        i += 1;
    }
    let mut old = String::new();
    while i < lines.len() && lines[i].trim() != "====" {
        if !old.is_empty() {
            old.push('\n');
        }
        old.push_str(lines[i]);
        i += 1;
    }
    if i < lines.len() && lines[i].trim() == "====" {
        i += 1;
    }
    let mut new = String::new();
    while i < lines.len() && lines[i].trim() != ">>>>" {
        if !new.is_empty() {
            new.push('\n');
        }
        new.push_str(lines[i]);
        i += 1;
    }
    if i < lines.len() && lines[i].trim() == ">>>>" {
        i += 1;
    }
    Ok((old, new, i))
}

fn parse_agent(rest: &str) -> Action {
    let rest = rest.trim();
    if rest.is_empty() {
        return Action::Agent {
            name: "worker".into(),
            goal: "help".into(),
        };
    }
    if let Some((name, goal)) = rest.split_once(':') {
        Action::Agent {
            name: slug(name),
            goal: goal.trim().to_string(),
        }
    } else if let Some((name, goal)) = rest.split_once(char::is_whitespace) {
        Action::Agent {
            name: slug(name),
            goal: goal.trim().to_string(),
        }
    } else {
        Action::Agent {
            name: slug(rest),
            goal: rest.to_string(),
        }
    }
}

fn slug(s: &str) -> String {
    let s: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "worker".into()
    } else {
        s.chars().take(24).collect()
    }
}

fn looks_like_shell(line: &str) -> bool {
    let t = line.trim();
    t.starts_with("ls")
        || t.starts_with("cat ")
        || t.starts_with("pwd")
        || t.starts_with("echo ")
        || t.starts_with("git ")
        || t.starts_with("python")
        || t.starts_with("cargo ")
        || t.starts_with("rg ")
        || t.starts_with("grep ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first(raw: &str) -> Action {
        parse_agent_step(raw)
            .actions
            .into_iter()
            .next()
            .expect("an action")
    }

    #[test]
    fn parses_qwen_xml_tool_call_as_read() {
        let raw = "<tool_call><function=read><parameter=file_path>src/main.rs</parameter></function></tool_call>";
        match first(raw) {
            Action::Read { path } => assert_eq!(path, "src/main.rs"),
            other => panic!("expected Read, got {other:?}"),
        }
    }

    #[test]
    fn xml_write_carries_contents() {
        let raw = "<function=write><parameter=path>index.html</parameter><parameter=content><h1>hi</h1></parameter></function>";
        match first(raw) {
            Action::Write { path, contents } => {
                assert_eq!(path, "index.html");
                assert!(contents.contains("hi"));
            }
            other => panic!("expected Write, got {other:?}"),
        }
    }

    #[test]
    fn prose_is_a_final_answer() {
        let step = parse_agent_step("I will read the goal file and then decide what to do next.");
        assert!(step.actions.is_empty());
        assert!(step.text.starts_with("I will read"));
    }

    #[test]
    fn plain_action_lines_with_blocks() {
        let step = parse_agent_step("Writing it.\nwrite a.txt\n<<\nhi\n>>\nrun cargo test");
        assert_eq!(step.text, "Writing it.");
        assert!(matches!(step.actions[0], Action::Write { .. }));
        assert!(matches!(step.actions[1], Action::Run { .. }));
    }

    #[test]
    fn mcp_and_skill_and_agent_calls_route() {
        let raw = "<function=mcp><parameter=tool>fs/read</parameter><parameter=args>{\"p\":1}</parameter></function>";
        assert!(matches!(first(raw), Action::Mcp { .. }));
        let raw = "<function=use_skill><parameter=name>pdf</parameter></function>";
        assert!(matches!(first(raw), Action::Skill { .. }));
        let raw = "<function=agent><parameter=type>explore</parameter><parameter=goal>map src</parameter></function>";
        assert!(matches!(first(raw), Action::Agent { .. }));
    }

    #[test]
    fn unknown_function_becomes_mcp_call() {
        let raw = "<function=weather__lookup><parameter=city>Cairo</parameter></function>";
        match first(raw) {
            Action::Mcp { tool, args } => {
                assert_eq!(tool, "weather/lookup");
                assert!(args.contains("Cairo"));
            }
            other => panic!("expected Mcp, got {other:?}"),
        }
    }
}
