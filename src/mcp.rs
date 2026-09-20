//! Minimal MCP client over stdio JSON-RPC 2.0.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct McpServerSpec {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct McpTool {
    pub server: String,
    pub name: String,
    pub description: String,
}

pub fn load_specs(root: &Path) -> Vec<McpServerSpec> {
    let mut paths = vec![root.join(".ice/mcp.json")];
    if let Ok(h) = std::env::var("HOME") {
        paths.push(std::path::PathBuf::from(h).join(".ice/mcp.json"));
    }
    for p in paths {
        if let Ok(txt) = std::fs::read_to_string(&p) {
            if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                return parse_specs(&v);
            }
        }
    }
    Vec::new()
}

fn parse_specs(v: &Value) -> Vec<McpServerSpec> {
    let mut out = Vec::new();
    let servers = v
        .get("mcpServers")
        .or_else(|| v.get("servers"))
        .cloned()
        .unwrap_or(Value::Null);
    match servers {
        Value::Object(map) => {
            for (name, spec) in map {
                let cmd = spec
                    .get("command")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                if cmd.is_empty() {
                    continue;
                }
                let args = spec
                    .get("args")
                    .and_then(|x| x.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                out.push(McpServerSpec {
                    name,
                    command: cmd,
                    args,
                });
            }
        }
        _ => {}
    }
    out
}

struct Session {
    child: Child,
    stdout: BufReader<std::process::ChildStdout>,
    next_id: u64,
}

impl Session {
    fn spawn(spec: &McpServerSpec) -> Result<Self> {
        let mut child = Command::new(&spec.command)
            .args(&spec.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("spawn mcp {}", spec.command))?;
        let stdout = child.stdout.take().context("mcp stdout")?;
        Ok(Self {
            child,
            stdout: BufReader::new(stdout),
            next_id: 1,
        })
    }

    fn rpc(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        let line = format!("{req}\n");
        let stdin = self.child.stdin.as_mut().context("mcp stdin")?;
        stdin.write_all(line.as_bytes())?;
        stdin.flush()?;

        let start = std::time::Instant::now();
        loop {
            if start.elapsed() > Duration::from_secs(8) {
                bail!("mcp timeout on {method}");
            }
            let mut buf = String::new();
            // Non-blocking-ish: try read a line; BufReader blocks.
            // For tests we use a fast local server.
            buf.clear();
            let n = self.stdout.read_line(&mut buf)?;
            if n == 0 {
                bail!("mcp closed");
            }
            let v: Value = serde_json::from_str(buf.trim()).unwrap_or(Value::Null);
            if v.get("id").and_then(|x| x.as_u64()) == Some(id)
                || v.get("id").and_then(|x| x.as_i64()) == Some(id as i64)
            {
                if let Some(err) = v.get("error") {
                    bail!("mcp error: {err}");
                }
                return Ok(v.get("result").cloned().unwrap_or(Value::Null));
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

pub fn list_tools(root: &Path) -> Result<Vec<McpTool>> {
    let mut tools = Vec::new();
    for spec in load_specs(root) {
        let mut s = Session::spawn(&spec)?;
        let _ = s.rpc(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "ice", "version": "0.1.0"}
            }),
        );
        let listed = s.rpc("tools/list", json!({}))?;
        if let Some(arr) = listed.get("tools").and_then(|t| t.as_array()) {
            for t in arr {
                tools.push(McpTool {
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
                });
            }
        }
    }
    Ok(tools)
}

pub fn call_tool(root: &Path, qualified: &str, args_json: &str) -> Result<String> {
    let (server, tool) = match qualified.split_once('/') {
        Some((s, t)) => (s.to_string(), t.to_string()),
        None => {
            let specs = load_specs(root);
            (
                specs
                    .first()
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| "default".into()),
                qualified.to_string(),
            )
        }
    };
    let spec = load_specs(root)
        .into_iter()
        .find(|s| s.name == server)
        .context("unknown mcp server")?;
    let mut s = Session::spawn(&spec)?;
    let _ = s.rpc(
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "ice", "version": "0.1.0"}
        }),
    );
    let args: Value = serde_json::from_str(args_json).unwrap_or(json!({}));
    let res = s.rpc("tools/call", json!({"name": tool, "arguments": args}))?;
    Ok(res.to_string())
}

pub fn catalog(root: &Path) -> String {
    match list_tools(root) {
        Ok(tools) if tools.is_empty() => {
            "no mcp tools. write .ice/mcp.json with mcpServers.{name}.command".into()
        }
        Ok(tools) => {
            let mut s = format!("{} mcp tool(s):\n", tools.len());
            for t in tools {
                s.push_str(&format!("  • {}/{}  {}\n", t.server, t.name, t.description));
            }
            s
        }
        Err(e) => format!("mcp list failed: {e}"),
    }
}

/// Dummy read so rustc does not warn on unused Read import in some editions.
#[allow(dead_code)]
fn _touch_read<R: Read>(_: &mut R) {}
