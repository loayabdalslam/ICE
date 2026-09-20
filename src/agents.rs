//! Subagents — Claude-Code-style delegated workers.
//!
//! Each subagent runs its own tight ICE loop in an ISOLATED context: it only
//! ever sees the goal delegated to it, never the parent transcript, so parallel
//! or exploratory work does not inflate the parent's token budget. What comes
//! back to the parent (and the board) is a COMPACT report — a one-line result,
//! the files it touched, and step/timing counts — while the full transcript is
//! written to `.ice/agents/<name>.md` for inspection.

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

/// A specialization a subagent can take. Mirrors Claude Code's agent types:
/// read-only explorers/reviewers that cannot touch the workspace, planners,
/// and full builders.
#[derive(Clone, Copy, Debug)]
pub struct Archetype {
    pub id: &'static str,
    pub blurb: &'static str,
    pub read_only: bool,
    pub preamble: &'static str,
}

pub const ARCHETYPES: &[Archetype] = &[
    Archetype {
        id: "explore",
        blurb: "read-only search across the workspace",
        read_only: true,
        preamble: "You are a READ-ONLY explorer. Use read/list/grep only to investigate and report what you find. Never write, replace, or run mutating commands.",
    },
    Archetype {
        id: "plan",
        blurb: "read-only; produces a step-by-step plan",
        read_only: true,
        preamble: "You are a PLANNER. Investigate read-only, then report a concise, ordered plan. Do not modify the workspace.",
    },
    Archetype {
        id: "review",
        blurb: "read-only code review / critique",
        read_only: true,
        preamble: "You are a REVIEWER. Read the relevant files and report concrete issues and fixes. Do not modify anything.",
    },
    Archetype {
        id: "build",
        blurb: "implements changes and writes files",
        read_only: false,
        preamble: "You are a BUILDER. Make the smallest set of edits that satisfies the goal, and assert the result.",
    },
    Archetype {
        id: "general",
        blurb: "general-purpose worker",
        read_only: false,
        preamble: "You are a general-purpose worker. Finish the delegated goal in as few bursts as possible.",
    },
];

/// Resolve a handle to an archetype by keyword (build/plan/review/explore),
/// defaulting to general-purpose.
pub fn resolve(name: &str) -> &'static Archetype {
    let n = name.to_ascii_lowercase();
    for a in ARCHETYPES {
        if n == a.id || n.contains(a.id) {
            return a;
        }
    }
    // Common synonyms.
    if n.contains("scout") || n.contains("search") || n.contains("find") || n.contains("map") {
        return &ARCHETYPES[0];
    }
    if n.contains("critic") || n.contains("audit") {
        return &ARCHETYPES[2];
    }
    if n.contains("writ") || n.contains("impl") || n.contains("code") || n.contains("fix") {
        return &ARCHETYPES[3];
    }
    &ARCHETYPES[4]
}

pub fn catalog() -> String {
    let mut s = String::from("subagent types:\n");
    for a in ARCHETYPES {
        s.push_str(&format!(
            "  • {:<8} {}{}\n",
            a.id,
            a.blurb,
            if a.read_only { " · read-only" } else { "" }
        ));
    }
    s.push_str("\nspawn:  /agent <type>: <goal>   or in a burst:  agent <type>: <goal>\n");
    s
}

/// Run a child ICE loop with a tight turn budget, an archetype role, and no
/// further spawning. Returns a compact report as the step output.
pub fn run_subagent(root: &Path, name: &str, goal: &str, demo: bool, max_turns: u32) -> StepResult {
    let t0 = Instant::now();
    let _ = DurableGoal::ensure_dir(root);
    let arch = resolve(name);
    match run_inner(root, name, arch, goal, demo, max_turns.max(1).min(3)) {
        Ok(report) => {
            let path = root.join(".ice/agents").join(format!("{name}.md"));
            let _ = fs::write(
                &path,
                format!(
                    "# subagent {name} ({})\n\n## goal\n{goal}\n\n## report\n{}\n\n## transcript\n{}\n",
                    arch.id, report.summary, report.transcript
                ),
            );
            StepResult {
                title: format!("agent {}·{name}", arch.id),
                ok: report.ok,
                exit: if report.ok { 0 } else { 1 },
                output: report.board(),
                ms: t0.elapsed().as_millis(),
            }
        }
        Err(e) => StepResult {
            title: format!("agent {}·{name}", arch.id),
            ok: false,
            exit: 1,
            output: format!("failed: {e}"),
            ms: t0.elapsed().as_millis(),
        },
    }
}

struct Report {
    ok: bool,
    summary: String,
    files: Vec<String>,
    steps: usize,
    transcript: String,
}

impl Report {
    /// The compact, board-facing view the parent sees — never the transcript.
    fn board(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("report · {}", one_line(&self.summary)));
        if !self.files.is_empty() {
            s.push_str(&format!("\nfiles: {}", self.files.join(", ")));
        }
        s.push_str(&format!("\n{} step(s)", self.steps));
        s
    }
}

fn run_inner(
    root: &Path,
    name: &str,
    arch: &Archetype,
    goal: &str,
    demo: bool,
    max_turns: u32,
) -> Result<Report> {
    let mut last_delta: Option<String> = None;
    let mut transcript = String::new();
    let mut files: Vec<String> = Vec::new();
    let mut total_steps = 0usize;
    let mut last_summary = String::new();

    for turn in 1..=max_turns {
        let raw = if demo {
            demo_child(name, goal, turn)
        } else {
            let llm = Llm::from_env().ok_or_else(|| anyhow::anyhow!("no API key"))?;
            // Isolated context: only the role + delegated goal + own last delta.
            let user = format!(
                "{}\n\nYou are subagent `{name}`.\nDelegated goal:\n{goal}\n\n{}\nEmit ONE BURST. Do not spawn further agents.",
                arch.preamble,
                last_delta.as_deref().unwrap_or("No prior delta."),
            );
            llm.complete(llm::SYSTEM_PROMPT, &user)?.content
        };
        transcript.push_str(&format!("-- {name} burst {turn} --\n{raw}\n"));
        let burst = Burst::parse(&raw)?;
        let mut steps = Vec::new();
        for action in &burst.actions {
            // No nested spawning.
            if matches!(action, Action::Agent { .. }) {
                continue;
            }
            // Read-only archetypes cannot mutate the workspace.
            if arch.read_only
                && matches!(
                    action,
                    Action::Write { .. } | Action::Replace { .. } | Action::Run { .. }
                )
            {
                steps.push(StepResult {
                    title: "blocked (read-only agent)".into(),
                    ok: true,
                    exit: 0,
                    output: "skipped a mutating action".into(),
                    ms: 0,
                });
                continue;
            }
            if let Action::Write { path, .. } = action {
                files.push(path.clone());
            }
            steps.push(exec::run_action(root, action));
        }
        total_steps += steps.len();
        let asserts = verify::check(root, &burst.asserts, &steps);
        let packed = delta::pack(&steps, &asserts);
        transcript.push_str(&packed.summary);
        transcript.push('\n');
        last_summary = packed.summary.clone();
        if packed.ok {
            return Ok(Report {
                ok: true,
                summary: packed.summary,
                files,
                steps: total_steps,
                transcript,
            });
        }
        // Stop retrying an identical failure.
        if last_delta.as_deref() == Some(packed.summary.as_str()) {
            break;
        }
        last_delta = Some(packed.summary);
    }
    Ok(Report {
        ok: false,
        summary: if last_summary.is_empty() {
            "no progress".into()
        } else {
            last_summary
        },
        files,
        steps: total_steps,
        transcript,
    })
}

fn one_line(s: &str) -> String {
    let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if line.chars().count() > 120 {
        format!("{}…", line.chars().take(119).collect::<String>())
    } else {
        line.to_string()
    }
}

fn demo_child(name: &str, goal: &str, turn: u32) -> String {
    if name.contains("write") || name.contains("build") {
        return format!(
            "GOAL: {goal}\nBURST:\n  write .ice/DONE\n  <<\nclosed by {name}\n  >>\nASSERT:\n  file_exists .ice/DONE\n"
        );
    }
    if turn == 1 {
        format!("GOAL: {goal}\nBURST:\n  list .\nASSERT:\n  exit 0\n")
    } else {
        format!("GOAL: {goal}\nBURST:\n  run echo subagent {name} finished\nASSERT:\n  exit 0\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archetypes_resolve_by_keyword() {
        assert_eq!(resolve("explore").id, "explore");
        assert_eq!(resolve("scout").id, "explore");
        assert_eq!(resolve("reviewer").id, "review");
        assert_eq!(resolve("builder").id, "build");
        assert_eq!(resolve("whatever").id, "general");
        assert!(resolve("explore").read_only);
        assert!(!resolve("build").read_only);
    }
}
