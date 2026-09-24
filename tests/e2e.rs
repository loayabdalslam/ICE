//! End-to-end tests: run the real `ice` binary in print mode against a
//! local mock of the OpenAI and Anthropic streaming APIs.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

/// One scripted assistant turn: text and/or tool calls (name, JSON args).
#[derive(Clone)]
struct Turn {
    text: &'static str,
    tools: Vec<(&'static str, &'static str)>,
}

struct Mock {
    port: u16,
    requests: Arc<Mutex<Vec<(String, String, serde_json::Value)>>>,
}

fn read_request(s: &mut TcpStream) -> (String, String, serde_json::Value) {
    let mut r = BufReader::new(s.try_clone().unwrap());
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
    let mut len = 0;
    let mut auth = String::new();
    loop {
        let mut h = String::new();
        r.read_line(&mut h).unwrap();
        let h = h.trim_end().to_string();
        if h.is_empty() {
            break;
        }
        let lower = h.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            len = v.trim().parse().unwrap();
        }
        if lower.starts_with("authorization:") || lower.starts_with("x-api-key:") {
            auth = h.split_once(':').unwrap().1.trim().to_string();
        }
    }
    let mut body = vec![0; len];
    r.read_exact(&mut body).unwrap();
    (
        path,
        auth,
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
    )
}

fn serve(turns: Vec<Turn>) -> Mock {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let reqs = requests.clone();
    std::thread::spawn(move || {
        for (n, stream) in l.incoming().enumerate() {
            let mut s = stream.unwrap();
            let (path, auth, body) = read_request(&mut s);
            reqs.lock().unwrap().push((path.clone(), auth, body));
            let turn = turns[n.min(turns.len() - 1)].clone();
            let mut out = String::from(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
            );
            if path.ends_with("/messages") {
                let mut ev = |t: &str, d: serde_json::Value| {
                    out.push_str(&format!("event: {t}\ndata: {d}\n\n"))
                };
                ev(
                    "message_start",
                    serde_json::json!({"type":"message_start","message":{"usage":{"input_tokens":10,"output_tokens":1}}}),
                );
                let mut idx = 0;
                if !turn.text.is_empty() {
                    ev(
                        "content_block_start",
                        serde_json::json!({"type":"content_block_start","index":idx,"content_block":{"type":"text","text":""}}),
                    );
                    ev(
                        "content_block_delta",
                        serde_json::json!({"type":"content_block_delta","index":idx,"delta":{"type":"text_delta","text":turn.text}}),
                    );
                    ev(
                        "content_block_stop",
                        serde_json::json!({"type":"content_block_stop","index":idx}),
                    );
                    idx += 1;
                }
                for (i, (name, args)) in turn.tools.iter().enumerate() {
                    ev(
                        "content_block_start",
                        serde_json::json!({"type":"content_block_start","index":idx,"content_block":{"type":"tool_use","id":format!("toolu_{n}_{i}"),"name":name,"input":{}}}),
                    );
                    ev(
                        "content_block_delta",
                        serde_json::json!({"type":"content_block_delta","index":idx,"delta":{"type":"input_json_delta","partial_json":args}}),
                    );
                    ev(
                        "content_block_stop",
                        serde_json::json!({"type":"content_block_stop","index":idx}),
                    );
                    idx += 1;
                }
                let stop = if turn.tools.is_empty() {
                    "end_turn"
                } else {
                    "tool_use"
                };
                ev(
                    "message_delta",
                    serde_json::json!({"type":"message_delta","delta":{"stop_reason":stop},"usage":{"output_tokens":5}}),
                );
                ev("message_stop", serde_json::json!({"type":"message_stop"}));
            } else {
                let mut ev = |d: serde_json::Value| out.push_str(&format!("data: {d}\n\n"));
                if !turn.text.is_empty() {
                    ev(serde_json::json!({"choices":[{"delta":{"content":turn.text}}]}));
                }
                for (i, (name, args)) in turn.tools.iter().enumerate() {
                    ev(
                        serde_json::json!({"choices":[{"delta":{"tool_calls":[{"index":i,"id":format!("call_{n}_{i}"),"function":{"name":name,"arguments":args}}]}}]}),
                    );
                }
                ev(
                    serde_json::json!({"choices":[{"delta":{},"finish_reason": if turn.tools.is_empty() {"stop"} else {"tool_calls"}}]}),
                );
                ev(
                    serde_json::json!({"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":10}}),
                );
                out.push_str("data: [DONE]\n\n");
            }
            let _ = s.write_all(out.as_bytes());
        }
    });
    Mock { port, requests }
}

fn tempdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "ice-e2e-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ice(
    project: &PathBuf,
    cfg: &PathBuf,
    env: &[(&str, String)],
    args: &[&str],
) -> (i32, String, String) {
    let mut c = Command::new(env!("CARGO_BIN_EXE_ice"));
    c.args(args)
        .current_dir(project)
        .stdin(Stdio::null())
        .env("ICE_CONFIG_DIR", cfg)
        .env("ICE_NO_AUTO_UPDATE", "1");
    for k in [
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        "XAI_API_KEY",
        "GROQ_API_KEY",
        "OPENROUTER_API_KEY",
        "ICE_PROVIDER",
        "ICE_MODEL",
        "ICE_BASE_URL",
        "ICE_API_KEY",
    ] {
        c.env_remove(k);
    }
    for (k, v) in env {
        c.env(k, v);
    }
    let o = c.output().unwrap();
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout).into(),
        String::from_utf8_lossy(&o.stderr).into(),
    )
}

fn openai_env(port: u16) -> Vec<(&'static str, String)> {
    vec![
        ("ICE_PROVIDER", "custom".into()),
        ("ICE_BASE_URL", format!("http://127.0.0.1:{port}/v1")),
        ("ICE_API_KEY", "sk-e2e-secret".into()),
        ("ICE_MODEL", "mock".into()),
    ]
}

#[test]
fn openai_tool_loop_writes_a_file_and_reports_json() {
    let mock = serve(vec![
        Turn {
            text: "Creating it.",
            tools: vec![("Write", r#"{"file_path":"hello.txt","content":"hi\n"}"#)],
        },
        Turn {
            text: "Done.",
            tools: vec![],
        },
    ]);
    let (proj, cfg) = (tempdir("oa"), tempdir("oa-cfg"));
    let (code, out, err) = ice(
        &proj,
        &cfg,
        &openai_env(mock.port),
        &[
            "-p",
            "make hello.txt",
            "--output-format",
            "json",
            "--allowedTools",
            "Write",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["type"], "result");
    assert_eq!(v["subtype"], "success");
    assert_eq!(v["result"], "Done.");
    assert_eq!(v["num_turns"], 2);
    assert_eq!(
        std::fs::read_to_string(proj.join("hello.txt")).unwrap(),
        "hi\n"
    );
    let reqs = mock.requests.lock().unwrap();
    assert_eq!(reqs[0].1, "Bearer sk-e2e-secret");
    let second = &reqs[1].2["messages"];
    let roles: Vec<&str> = second
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, vec!["system", "user", "assistant", "tool"]);
    // The session was saved for --continue.
    assert!(cfg.join("projects").exists());
}

#[test]
fn permission_is_denied_without_allow_rule_in_print_mode() {
    let mock = serve(vec![
        Turn {
            text: "",
            tools: vec![("Bash", r#"{"command":"touch pwned"}"#)],
        },
        Turn {
            text: "Could not run it.",
            tools: vec![],
        },
    ]);
    let (proj, cfg) = (tempdir("deny"), tempdir("deny-cfg"));
    let (code, out, _) = ice(&proj, &cfg, &openai_env(mock.port), &["-p", "run it"]);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "Could not run it.");
    assert!(!proj.join("pwned").exists());
    let reqs = mock.requests.lock().unwrap();
    let tool_msg = reqs[1].2["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert!(tool_msg["content"]
        .as_str()
        .unwrap()
        .contains("not granted"));
}

#[test]
fn anthropic_stream_with_caching_and_aliases() {
    let mock = serve(vec![
        Turn {
            text: "Reading.",
            tools: vec![("Read", r#"{"file_path":"a.txt"}"#)],
        },
        Turn {
            text: "It says hello.",
            tools: vec![],
        },
    ]);
    let (proj, cfg) = (tempdir("an"), tempdir("an-cfg"));
    std::fs::write(proj.join("a.txt"), "hello\n").unwrap();
    let env = vec![
        ("ICE_PROVIDER", "anthropic".to_string()),
        ("ICE_BASE_URL", format!("http://127.0.0.1:{}/v1", mock.port)),
        ("ANTHROPIC_API_KEY", "sk-ant-e2e".into()),
        ("ICE_MODEL", "sonnet".into()),
    ];
    let (code, out, err) = ice(&proj, &cfg, &env, &["-p", "what does a.txt say?"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim(), "It says hello.");
    let reqs = mock.requests.lock().unwrap();
    let body = &reqs[0].2;
    assert_eq!(reqs[0].1, "sk-ant-e2e");
    assert_eq!(body["model"], "claude-sonnet-4-5");
    assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
    let result = &reqs[1].2["messages"][2]["content"][0];
    assert_eq!(result["type"], "tool_result");
    assert!(result["content"]
        .as_str()
        .unwrap()
        .contains("     1\thello"));
}

#[test]
fn continue_restores_the_previous_conversation() {
    let mock = serve(vec![
        Turn {
            text: "Noted.",
            tools: vec![],
        },
        Turn {
            text: "It was 42.",
            tools: vec![],
        },
    ]);
    let (proj, cfg) = (tempdir("cont"), tempdir("cont-cfg"));
    let env = openai_env(mock.port);
    assert_eq!(ice(&proj, &cfg, &env, &["-p", "remember 42"]).0, 0);
    let (code, out, _) = ice(&proj, &cfg, &env, &["-p", "-c", "what was it?"]);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "It was 42.");
    let reqs = mock.requests.lock().unwrap();
    let msgs = reqs[1].2["messages"].as_array().unwrap();
    assert_eq!(msgs[1]["content"], "remember 42");
    assert_eq!(msgs[2]["content"], "Noted.");
}

#[test]
fn subcommands_work_without_a_model() {
    let (proj, cfg) = (tempdir("sub"), tempdir("sub-cfg"));
    let (code, out, _) = ice(&proj, &cfg, &[], &["--version"]);
    assert_eq!(code, 0);
    assert!(out.contains(env!("CARGO_PKG_VERSION")));
    let (code, out, _) = ice(&proj, &cfg, &[], &["init"]);
    assert_eq!(code, 0, "{out}");
    assert!(proj.join(".ice/settings.json").exists());
    let (code, _, _) = ice(
        &proj,
        &cfg,
        &[],
        &["mcp", "add", "echo", "--", "python3", "server.py"],
    );
    assert_eq!(code, 0);
    assert!(std::fs::read_to_string(proj.join(".ice/mcp.json"))
        .unwrap()
        .contains("server.py"));
    let (code, out, _) = ice(&proj, &cfg, &[], &["completions", "bash"]);
    assert_eq!(code, 0);
    assert!(out.contains("ice"));
    let (code, _, err) = ice(&proj, &cfg, &[], &["-p", "hi"]);
    assert_eq!(code, 1);
    assert!(err.contains("No model is configured"), "{err}");
}
