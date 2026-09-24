//! MCP client: stdio and streamable-HTTP transports, a persistent server
//! pool, and tool discovery with input schemas. Config is read from (in
//! order, later wins per server name):
//!   ~/.ice/mcp.json, <project>/.mcp.json, <project>/.ice/mcp.json
//! using the common `{"mcpServers": {name: {command,args,env} | {type,url,headers}}}` shape.

use crate::model::ToolSpec;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::Mutex;
use std::time::Duration;

const PROTOCOL: &str = "2025-03-26";

#[derive(Clone, Debug, PartialEq)]
pub enum Transport {
    Stdio {
        command: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
    },
    Http {
        url: String,
        headers: Vec<(String, String)>,
    },
}

#[derive(Clone, Debug)]
pub struct ServerSpec {
    pub name: String,
    pub transport: Transport,
    pub source: String,
}

#[derive(Clone, Debug)]
pub struct McpTool {
    pub server: String,
    pub name: String,
    pub description: String,
    pub schema: Value,
}

impl McpTool {
    pub fn qualified(&self) -> String {
        format!("mcp__{}__{}", sanitize(&self.server), sanitize(&self.name))
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn expand_env(s: &str) -> String {
    let re = regex::Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)(?::-([^}]*))?\}").unwrap();
    re.replace_all(s, |c: &regex::Captures| {
        std::env::var(&c[1])
            .unwrap_or_else(|_| c.get(2).map(|m| m.as_str().to_string()).unwrap_or_default())
    })
    .into_owned()
}

pub fn config_paths(root: &Path) -> Vec<(std::path::PathBuf, &'static str)> {
    vec![
        (crate::settings::user_dir().join("mcp.json"), "user"),
        (root.join(".mcp.json"), "project"),
        (root.join(".ice/mcp.json"), "local"),
    ]
}

pub fn load_specs(root: &Path) -> Vec<ServerSpec> {
    let mut by_name: Vec<ServerSpec> = Vec::new();
    for (p, source) in config_paths(root) {
        let Ok(txt) = std::fs::read_to_string(&p) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(&txt) else {
            continue;
        };
        for s in parse_specs(&v, source) {
            by_name.retain(|x| x.name != s.name);
            by_name.push(s);
        }
    }
    by_name
}

fn parse_specs(v: &Value, source: &str) -> Vec<ServerSpec> {
    let mut out = Vec::new();
    let Some(map) = v
        .get("mcpServers")
        .or_else(|| v.get("servers"))
        .and_then(|s| s.as_object())
    else {
        return out;
    };
    for (name, spec) in map {
        if spec
            .get("disabled")
            .and_then(|d| d.as_bool())
            .unwrap_or(false)
        {
            continue;
        }
        let kind = spec.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if let Some(url) = spec
            .get("url")
            .and_then(|u| u.as_str())
            .filter(|_| kind != "stdio")
        {
            let headers = spec
                .get("headers")
                .and_then(|h| h.as_object())
                .map(|o| {
                    o.iter()
                        .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), expand_env(v))))
                        .collect()
                })
                .unwrap_or_default();
            out.push(ServerSpec {
                name: name.clone(),
                transport: Transport::Http {
                    url: expand_env(url),
                    headers,
                },
                source: source.into(),
            });
            continue;
        }
        let Some(cmd) = spec
            .get("command")
            .and_then(|c| c.as_str())
            .filter(|c| !c.is_empty())
        else {
            continue;
        };
        let args = spec
            .get("args")
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(expand_env))
                    .collect()
            })
            .unwrap_or_default();
        let env = spec
            .get("env")
            .and_then(|e| e.as_object())
            .map(|o| {
                o.iter()
                    .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), expand_env(v))))
                    .collect()
            })
            .unwrap_or_default();
        out.push(ServerSpec {
            name: name.clone(),
            transport: Transport::Stdio {
                command: expand_env(cmd),
                args,
                env,
            },
            source: source.into(),
        });
    }
    out
}

enum Conn {
    Stdio {
        child: Child,
        stdin: ChildStdin,
        rx: Receiver<Value>,
    },
    Http {
        url: String,
        headers: Vec<(String, String)>,
        session: Option<String>,
    },
}

struct Server {
    conn: Conn,
    next_id: u64,
}

impl Server {
    fn start(spec: &ServerSpec) -> Result<Self> {
        let conn = match &spec.transport {
            Transport::Stdio { command, args, env } => {
                let mut cmd = Command::new(command);
                cmd.args(args)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null());
                for (k, v) in env {
                    cmd.env(k, v);
                }
                let mut child = cmd.spawn().with_context(|| {
                    format!("failed to start MCP server `{}` ({command})", spec.name)
                })?;
                let stdin = child.stdin.take().context("mcp stdin")?;
                let stdout = child.stdout.take().context("mcp stdout")?;
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    for line in BufReader::new(stdout).lines() {
                        let Ok(line) = line else { break };
                        if let Ok(v) = serde_json::from_str::<Value>(line.trim()) {
                            if tx.send(v).is_err() {
                                break;
                            }
                        }
                    }
                });
                Conn::Stdio { child, stdin, rx }
            }
            Transport::Http { url, headers } => Conn::Http {
                url: url.clone(),
                headers: headers.clone(),
                session: None,
            },
        };
        let mut s = Server { conn, next_id: 1 };
        s.rpc(
            "initialize",
            json!({"protocolVersion": PROTOCOL, "capabilities": {}, "clientInfo": {"name": "ice", "version": env!("CARGO_PKG_VERSION")}}),
            Duration::from_secs(30),
        )?;
        s.notify("notifications/initialized")?;
        Ok(s)
    }

    fn notify(&mut self, method: &str) -> Result<()> {
        let msg = json!({"jsonrpc": "2.0", "method": method});
        match &mut self.conn {
            Conn::Stdio { stdin, .. } => {
                stdin.write_all(format!("{msg}\n").as_bytes())?;
                stdin.flush()?;
            }
            Conn::Http { .. } => {
                let _ = self.http_post(&msg);
            }
        }
        Ok(())
    }

    fn http_post(&mut self, msg: &Value) -> Result<Option<Value>> {
        let Conn::Http {
            url,
            headers,
            session,
        } = &mut self.conn
        else {
            unreachable!()
        };
        let mut h: Vec<(String, String)> = headers.clone();
        h.push((
            "accept".into(),
            "application/json, text/event-stream".into(),
        ));
        if let Some(sid) = session.as_ref() {
            h.push(("mcp-session-id".into(), sid.clone()));
        }
        let resp = crate::http::post(
            &crate::http::Request {
                url,
                headers: h,
                body: msg,
                max_retries: 1,
            },
            None,
            &|_, _, _| {},
        )?;
        if let Some(sid) = resp.header("mcp-session-id") {
            *session = Some(sid.to_string());
        }
        let want = msg.get("id").cloned();
        if resp.content_type().contains("event-stream") {
            let mut found = None;
            crate::http::read_sse(resp, None, |_, data| {
                if let Ok(v) = serde_json::from_str::<Value>(data) {
                    if v.get("id") == want.as_ref() && found.is_none() {
                        found = Some(v);
                    }
                }
                Ok(())
            })?;
            return Ok(found);
        }
        let body = resp.into_string().unwrap_or_default();
        if body.trim().is_empty() {
            return Ok(None);
        }
        Ok(serde_json::from_str(&body).ok())
    }

    fn rpc(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let reply = match &mut self.conn {
            Conn::Stdio { stdin, rx, .. } => {
                stdin
                    .write_all(format!("{msg}\n").as_bytes())
                    .context("MCP server closed its input")?;
                stdin.flush()?;
                let deadline = std::time::Instant::now() + timeout;
                loop {
                    let left = deadline.saturating_duration_since(std::time::Instant::now());
                    if left.is_zero() {
                        bail!(
                            "MCP request `{method}` timed out after {}s",
                            timeout.as_secs()
                        );
                    }
                    match rx.recv_timeout(left) {
                        Ok(v) if v.get("id").and_then(|x| x.as_u64()) == Some(id) => break v,
                        Ok(_) => continue, // notifications / other replies
                        Err(mpsc::RecvTimeoutError::Timeout) => bail!(
                            "MCP request `{method}` timed out after {}s",
                            timeout.as_secs()
                        ),
                        Err(_) => bail!("MCP server exited"),
                    }
                }
            }
            Conn::Http { .. } => self
                .http_post(&msg)?
                .ok_or_else(|| anyhow!("MCP server sent no reply to `{method}`"))?,
        };
        if let Some(err) = reply.get("error") {
            bail!(
                "MCP error: {}",
                err.get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or(&err.to_string())
            );
        }
        Ok(reply.get("result").cloned().unwrap_or(Value::Null))
    }

    fn alive(&mut self) -> bool {
        match &mut self.conn {
            Conn::Stdio { child, .. } => matches!(child.try_wait(), Ok(None)),
            Conn::Http { .. } => true,
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Conn::Stdio { child, .. } = &mut self.conn {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Connected servers, keyed by name, shared for the process lifetime.
pub struct Pool {
    root: std::path::PathBuf,
    servers: Mutex<HashMap<String, Server>>,
    pub tools: Vec<McpTool>,
    pub status: Vec<(String, Result<usize, String>)>,
}

impl Pool {
    /// Connect to every configured server and list its tools. Failures are
    /// recorded per server (shown by /mcp) rather than aborting.
    pub fn connect(root: &Path) -> Pool {
        let specs = load_specs(root);
        let mut servers = HashMap::new();
        let mut tools = Vec::new();
        let mut status = Vec::new();
        // Start servers in parallel: slow npx servers shouldn't serialize.
        type Started = (ServerSpec, Result<(Server, Vec<McpTool>)>);
        let started: Vec<Started> = std::thread::scope(|sc| {
            let hs: Vec<_> = specs
                .iter()
                .map(|spec| {
                    sc.spawn(move || {
                        let r = (|| {
                            let mut s = Server::start(spec)?;
                            let listed = s.rpc("tools/list", json!({}), Duration::from_secs(30))?;
                            let mut ts = Vec::new();
                            for t in listed
                                .get("tools")
                                .and_then(|t| t.as_array())
                                .cloned()
                                .unwrap_or_default()
                            {
                                ts.push(McpTool {
                                    server: spec.name.clone(),
                                    name: t
                                        .get("name")
                                        .and_then(|x| x.as_str())
                                        .unwrap_or("tool")
                                        .into(),
                                    description: t
                                        .get("description")
                                        .and_then(|x| x.as_str())
                                        .unwrap_or("")
                                        .into(),
                                    schema: t
                                        .get("inputSchema")
                                        .cloned()
                                        .unwrap_or_else(|| json!({"type": "object"})),
                                });
                            }
                            Ok((s, ts))
                        })();
                        (spec.clone(), r)
                    })
                })
                .collect();
            hs.into_iter().filter_map(|h| h.join().ok()).collect()
        });
        for (spec, r) in started {
            match r {
                Ok((s, ts)) => {
                    status.push((spec.name.clone(), Ok(ts.len())));
                    tools.extend(ts);
                    servers.insert(spec.name.clone(), s);
                }
                Err(e) => status.push((spec.name.clone(), Err(e.to_string()))),
            }
        }
        Pool {
            root: root.to_path_buf(),
            servers: Mutex::new(servers),
            tools,
            status,
        }
    }

    pub fn empty(root: &Path) -> Pool {
        Pool {
            root: root.to_path_buf(),
            servers: Mutex::new(HashMap::new()),
            tools: Vec::new(),
            status: Vec::new(),
        }
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools
            .iter()
            .map(|t| ToolSpec {
                name: t.qualified(),
                description: format!("{} (MCP server: {})", t.description, t.server),
                input_schema: t.schema.clone(),
            })
            .collect()
    }

    pub fn find(&self, qualified: &str) -> Option<&McpTool> {
        self.tools.iter().find(|t| t.qualified() == qualified)
    }

    /// Call a tool; restarts a crashed stdio server once.
    pub fn call(&self, qualified: &str, args: &Value) -> Result<(String, bool)> {
        let tool = self
            .find(qualified)
            .ok_or_else(|| anyhow!("unknown MCP tool {qualified}"))?
            .clone();
        let mut servers = self.servers.lock().unwrap();
        let needs_restart = servers
            .get_mut(&tool.server)
            .map(|s| !s.alive())
            .unwrap_or(true);
        if needs_restart {
            let spec = load_specs(&self.root)
                .into_iter()
                .find(|s| s.name == tool.server)
                .ok_or_else(|| anyhow!("MCP server {} is no longer configured", tool.server))?;
            servers.insert(tool.server.clone(), Server::start(&spec)?);
        }
        let s = servers.get_mut(&tool.server).unwrap();
        let timeout = std::env::var("MCP_TOOL_TIMEOUT")
            .ok()
            .and_then(|t| t.parse().ok())
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_secs(300));
        let res = s.rpc(
            "tools/call",
            json!({"name": tool.name, "arguments": args}),
            timeout,
        )?;
        let is_error = res
            .get("isError")
            .and_then(|e| e.as_bool())
            .unwrap_or(false);
        let mut text = String::new();
        if let Some(items) = res.get("content").and_then(|c| c.as_array()) {
            for it in items {
                match it.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        text.push_str(it.get("text").and_then(|t| t.as_str()).unwrap_or(""));
                        text.push('\n');
                    }
                    Some("image") => text.push_str("[image]\n"),
                    Some("resource") => {
                        let r = &it["resource"];
                        text.push_str(
                            r.get("text").and_then(|t| t.as_str()).unwrap_or(
                                r.get("uri")
                                    .and_then(|u| u.as_str())
                                    .unwrap_or("[resource]"),
                            ),
                        );
                        text.push('\n');
                    }
                    _ => text.push_str(&it.to_string()),
                }
            }
        } else if let Some(sc) = res.get("structuredContent") {
            text = sc.to_string();
        } else {
            text = res.to_string();
        }
        Ok((text.trim_end().to_string(), is_error))
    }
}

/// Human summary for /mcp and `ice mcp list`.
pub fn describe(pool: &Pool, root: &Path) -> String {
    let specs = load_specs(root);
    if specs.is_empty() {
        return "No MCP servers configured.\nAdd one with `ice mcp add <name> -- <command> [args…]`, or create .mcp.json:\n  {\"mcpServers\": {\"name\": {\"command\": \"npx\", \"args\": [\"-y\", \"pkg\"]}}}".into();
    }
    let mut s = String::from("MCP servers:\n");
    for spec in &specs {
        let st = pool.status.iter().find(|(n, _)| n == &spec.name);
        let (mark, detail) = match st {
            Some((_, Ok(n))) => (
                "✔",
                format!("connected · {n} tool{}", if *n == 1 { "" } else { "s" }),
            ),
            Some((_, Err(e))) => ("✘", format!("failed · {e}")),
            None => ("○", "not connected (restart to connect)".into()),
        };
        let what = match &spec.transport {
            Transport::Stdio { command, args, .. } => format!("{command} {}", args.join(" ")),
            Transport::Http { url, .. } => url.clone(),
        };
        s.push_str(&format!(
            "  {mark} {} ({}) — {detail}\n      {}\n",
            spec.name,
            spec.source,
            what.trim()
        ));
    }
    for t in &pool.tools {
        s.push_str(&format!("    · {}\n", t.qualified()));
    }
    s
}

/// Add a stdio server to a config file (`ice mcp add`).
pub fn add_server(path: &Path, name: &str, transport: Transport) -> Result<()> {
    let mut v: Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}));
    if !v["mcpServers"].is_object() {
        v["mcpServers"] = json!({});
    }
    v["mcpServers"][name] = match transport {
        Transport::Stdio { command, args, env } => {
            let mut o = json!({"command": command, "args": args});
            if !env.is_empty() {
                o["env"] = Value::Object(
                    env.into_iter()
                        .map(|(k, v)| (k, Value::String(v)))
                        .collect(),
                );
            }
            o
        }
        Transport::Http { url, headers } => {
            let mut o = json!({"type": "http", "url": url});
            if !headers.is_empty() {
                o["headers"] = Value::Object(
                    headers
                        .into_iter()
                        .map(|(k, v)| (k, Value::String(v)))
                        .collect(),
                );
            }
            o
        }
    };
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(&v)? + "\n")?;
    Ok(())
}

pub fn remove_server(root: &Path, name: &str) -> Result<Vec<String>> {
    let mut removed = Vec::new();
    for (p, _) in config_paths(root) {
        let Ok(txt) = std::fs::read_to_string(&p) else {
            continue;
        };
        let Ok(mut v) = serde_json::from_str::<Value>(&txt) else {
            continue;
        };
        if let Some(o) = v.get_mut("mcpServers").and_then(|o| o.as_object_mut()) {
            if o.remove(name).is_some() {
                std::fs::write(&p, serde_json::to_string_pretty(&v)? + "\n")?;
                removed.push(p.display().to_string());
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stdio_and_http_specs_with_env_expansion() {
        std::env::set_var("ICE_TEST_TOKEN", "abc");
        let v = json!({"mcpServers": {
            "fs": {"command": "npx", "args": ["-y", "server-fs", "${HOME_NOT_SET:-/tmp}"]},
            "gh": {"type": "http", "url": "https://x/mcp", "headers": {"Authorization": "Bearer ${ICE_TEST_TOKEN}"}},
            "off": {"command": "x", "disabled": true}
        }});
        let s = parse_specs(&v, "project");
        assert_eq!(s.len(), 2);
        let fs = s.iter().find(|x| x.name == "fs").unwrap();
        assert!(matches!(&fs.transport, Transport::Stdio { args, .. } if args[2] == "/tmp"));
        let gh = s.iter().find(|x| x.name == "gh").unwrap();
        assert!(
            matches!(&gh.transport, Transport::Http { headers, .. } if headers[0].1 == "Bearer abc")
        );
    }

    #[test]
    fn qualified_names_are_sanitized() {
        let t = McpTool {
            server: "my server".into(),
            name: "do.thing".into(),
            description: String::new(),
            schema: json!({}),
        };
        assert_eq!(t.qualified(), "mcp__my_server__do_thing");
    }
}
