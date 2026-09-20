use crate::delta;
use crate::exec::{self, StepResult};
use crate::goal::DurableGoal;
use crate::ir::{Action, Burst};
use crate::llm::{self, Llm};
use crate::verify;
use anyhow::Result;
use std::fs;
use std::path::Path;
use std::time::Instant;

/// Run a child ICE loop with a tight turn budget and no further spawning.
pub fn run_subagent(root: &Path, name: &str, goal: &str, demo: bool, max_turns: u32) -> StepResult {
    let t0 = Instant::now();
    let _ = DurableGoal::ensure_dir(root);
    match run_inner(root, name, goal, demo, max_turns.max(1).min(3)) {
        Ok((ok, summary)) => {
            let path = root.join(".ice/agents").join(format!("{name}.md"));
            let _ = fs::write(
                &path,
                format!("# subagent {name}\n\n## goal\n{goal}\n\n## result\n{summary}\n"),
            );
            StepResult {
                title: format!("agent {name}"),
                ok,
                exit: if ok { 0 } else { 1 },
                output: summary,
                ms: t0.elapsed().as_millis(),
            }
        }
        Err(e) => StepResult {
            title: format!("agent {name}"),
            ok: false,
            exit: 1,
            output: e.to_string(),
            ms: t0.elapsed().as_millis(),
        },
    }
}

fn run_inner(
    root: &Path,
    name: &str,
    goal: &str,
    demo: bool,
    max_turns: u32,
) -> Result<(bool, String)> {
    let mut last_delta: Option<String> = None;
    let mut log = String::new();
    for turn in 1..=max_turns {
        let raw = if demo {
            demo_child(name, goal, turn)
        } else {
            let llm = Llm::from_env().ok_or_else(|| anyhow::anyhow!("no API key"))?;
            let user = format!(
                "You are subagent `{name}`.\nParent delegated this goal:\n{goal}\n\n{}\nEmit a BURST. Do not spawn further agents.",
                last_delta
                    .as_deref()
                    .unwrap_or("No prior delta.")
            );
            llm.complete(llm::SYSTEM_PROMPT, &user)?.content
        };
        log.push_str(&format!("-- {name} burst {turn} --\n{raw}\n"));
        let burst = Burst::parse(&raw)?;
        let mut steps = Vec::new();
        for action in &burst.actions {
            if matches!(action, Action::Agent { .. }) {
                steps.push(StepResult {
                    title: format!("agent {name} refused nested spawn"),
                    ok: true,
                    exit: 0,
                    output: "nested spawn skipped".into(),
                    ms: 0,
                });
                continue;
            }
            steps.push(exec::run_action(root, action));
        }
        let asserts = verify::check(root, &burst.asserts, &steps);
        let packed = delta::pack(&steps, &asserts);
        log.push_str(&packed.summary);
        log.push('\n');
        if packed.ok {
            return Ok((true, log));
        }
        last_delta = Some(packed.summary);
    }
    Ok((false, log))
}

fn demo_child(name: &str, goal: &str, turn: u32) -> String {
    if name.contains("write") {
        return format!(
            "GOAL: {goal}\nBURST:\n  write .ice/DONE\n  <<\nclosed by {name}\n  >>\nASSERT:\n  file_exists .ice/DONE\n"
        );
    }
    if turn == 1 {
        format!(
            "GOAL: {goal}\nBURST:\n  run echo subagent {name} exploring\n  list .\nASSERT:\n  exit 0\n"
        )
    } else {
        format!("GOAL: {goal}\nBURST:\n  run echo subagent {name} finished\nASSERT:\n  exit 0\n")
    }
}
