//! CLI-agent backend: drive an installed coding CLI (Codex/ChatGPT, Claude
//! Code, opencode, Gemini, Qwen…) as ICE's model, reusing the user's own login
//! in that tool instead of an API key. ICE sends the message to the CLI in its
//! non-interactive mode and shows the result.

use crate::providers::CliAgent;
use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Run the CLI agent once with the given prompt, in `root`. Returns its output.
pub fn complete(
    agent: &CliAgent,
    prompt: &str,
    root: &Path,
    cancel: &AtomicBool,
) -> Result<String> {
    let args: Vec<String> = agent
        .args
        .iter()
        .map(|a| a.replace("{PROMPT}", prompt))
        .collect();
    // A hard cap so a hung CLI can't freeze ICE forever.
    let timeout = std::env::var("ICE_CLI_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(240);
    let mut child = Command::new(agent.bin)
        .args(&args)
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "failed to launch `{}` — is it installed and on PATH? {}",
                agent.bin, agent.login_hint
            )
        })?;

    // Drain both pipes on threads so a chatty CLI can't block on a full pipe.
    let drain = |r: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut buf);
            }
            buf
        })
    };
    let h_out = drain(
        child
            .stdout
            .take()
            .map(|o| Box::new(o) as Box<dyn std::io::Read + Send>),
    );
    let h_err = drain(
        child
            .stderr
            .take()
            .map(|e| Box::new(e) as Box<dyn std::io::Read + Send>),
    );
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            bail!(crate::http::Interrupted);
        }
        if start.elapsed() > Duration::from_secs(timeout) {
            let _ = child.kill();
            let _ = child.wait();
            bail!("`{}` timed out after {timeout}s", agent.bin);
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let stdout = String::from_utf8_lossy(&h_out.join().unwrap_or_default())
        .trim()
        .to_string();
    let stderr = String::from_utf8_lossy(&h_err.join().unwrap_or_default())
        .trim()
        .to_string();
    struct Out {
        status: std::process::ExitStatus,
    }
    let out = Out { status };
    if !out.status.success() && stdout.is_empty() {
        let hint = if stderr.to_lowercase().contains("login")
            || stderr.to_lowercase().contains("auth")
            || stderr.to_lowercase().contains("api key")
        {
            format!("  {}", agent.login_hint)
        } else {
            String::new()
        };
        bail!(
            "`{}` failed: {}{hint}",
            agent.bin,
            if stderr.is_empty() {
                "no output"
            } else {
                &stderr
            }
        );
    }
    if stdout.is_empty() {
        Ok(stderr)
    } else {
        Ok(stdout)
    }
}
