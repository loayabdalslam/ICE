use crate::agents;
use crate::delta::{self, Delta};
use crate::exec::{self, StepResult};
use crate::goal::DurableGoal;
use crate::ir::{Action, Burst};
use crate::llm::{self, Llm};
use crate::verify;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::mpsc::Sender;

#[derive(Debug, Clone)]
pub enum Event {
    Thinking,
    ThinkingText(String),
    BurstSource(String),
    Step(StepResult),
    Asserts(Vec<(String, bool, String)>),
    Delta(Delta),
    Assistant(String),
    Loop {
        pass: u32,
        max: u32,
        note: String,
    },
    AgentStart {
        name: String,
        goal: String,
    },
    AgentDone {
        name: String,
        ok: bool,
        summary: String,
    },
    Goal {
        text: String,
        done: bool,
    },
    Done {
        turns: u32,
        ok: bool,
    },
    Error(String),
    Tokens {
        input: u32,
        output: u32,
    },
    TodoChanged,
}

pub struct Job {
    pub root: PathBuf,
    pub goal: String,
    pub prior_delta: Option<String>,
    pub max_turns: u32,
    pub demo: bool,
    pub loop_mode: bool,
    pub chat_mode: bool,
    pub durable: Option<DurableGoal>,
    pub tx: Sender<Event>,
}

pub fn run(job: Job) {
    let send = |e: Event| {
        let _ = job.tx.send(e);
    };
    if let Err(e) = run_inner(&job, &send) {
        send(Event::Error(e.to_string()));
    }
}

fn run_inner(job: &Job, send: &dyn Fn(Event)) -> Result<()> {
    if job.chat_mode {
        return run_chat(job, send);
    }
    if job.loop_mode {
        run_outer_loop(job, send)
    } else {
        run_ice_turns(
            job,
            send,
            &job.goal,
            job.prior_delta.clone(),
            job.max_turns,
            None,
        )
        .map(|_| ())
    }
}

/// Ralph-style outer loop: state on disk, fresh model context each pass.
fn run_outer_loop(job: &Job, send: &dyn Fn(Event)) -> Result<()> {
    let mut durable = job
        .durable
        .clone()
        .unwrap_or_else(|| DurableGoal::new(&job.goal, vec![], job.max_turns));
    durable.save(&job.root)?;
    DurableGoal::reset_progress(&job.root)?;
    send(Event::Goal {
        text: durable.text.clone(),
        done: false,
    });

    let max = durable.max_loops.max(1);
    for pass in 1..=max {
        if durable.is_done(&job.root) {
            durable.open = false;
            durable.save(&job.root)?;
            send(Event::Goal {
                text: durable.text.clone(),
                done: true,
            });
            send(Event::Done {
                turns: pass - 1,
                ok: true,
            });
            return Ok(());
        }
        send(Event::Loop {
            pass,
            max,
            note: format!("outer loop pass {pass}/{max}"),
        });
        let progress = DurableGoal::read_progress(&job.root);
        let prior = if progress.trim().is_empty() {
            None
        } else {
            Some(progress)
        };
        let inner_goal = format!(
            "{}\n\nDurable DONE WHEN lives in .ice/goal.md.\nWhen the goal is truly finished, write an empty marker to .ice/DONE and satisfy the DONE WHEN asserts.\nUse spawn/agent for parallel work.",
            durable.text
        );
        let _ = run_ice_turns(job, send, &inner_goal, prior, 3, Some(pass));
        let packed_note = format!(
            "## loop {pass}\n{}\n",
            DurableGoal::read_progress(&job.root)
        );
        let _ = packed_note;
        let any_ok = durable.is_done(&job.root);
        if any_ok {
            durable.open = false;
            durable.save(&job.root)?;
            send(Event::Goal {
                text: durable.text.clone(),
                done: true,
            });
            send(Event::Assistant(format!(
                "Goal closed on loop pass {pass}."
            )));
            send(Event::Done {
                turns: pass,
                ok: true,
            });
            return Ok(());
        }
    }
    send(Event::Assistant(
        "Loop budget exhausted. Goal still open. Inspect .ice/progress.md.".into(),
    ));
    send(Event::Done {
        turns: max,
        ok: false,
    });
    Ok(())
}

/// Fast conversational path: one completion, plain text, no burst/verify.
fn run_chat(job: &Job, send: &dyn Fn(Event)) -> Result<()> {
    if job.demo {
        send(Event::Assistant(demo_chat(&job.goal)));
        send(Event::Done { turns: 1, ok: true });
        return Ok(());
    }
    let llm = Llm::from_env()
        .ok_or_else(|| anyhow::anyhow!("no API key. Run /onboard or set a provider key."))?;
    let comp = llm.complete(llm::CHAT_SYSTEM, &job.goal)?;
    if !comp.thinking.trim().is_empty() {
        send(Event::ThinkingText(comp.thinking.clone()));
    }
    send(Event::Tokens {
        input: crate::theme::estimate_tokens(&job.goal)
            + crate::theme::estimate_tokens(llm::CHAT_SYSTEM),
        output: crate::theme::estimate_tokens(&comp.content),
    });
    let text = if comp.content.trim().is_empty() {
        "…".to_string()
    } else {
        comp.content
    };
    send(Event::Assistant(text));
    send(Event::Done { turns: 1, ok: true });
    Ok(())
}

fn demo_chat(goal: &str) -> String {
    let g = goal.trim().to_ascii_lowercase();
    if g.is_empty() {
        return "Hey — I'm ICE. What are we building?".into();
    }
    if ["hi", "hey", "hello", "yo", "sup", "hola"]
        .iter()
        .any(|h| g == *h || g.starts_with(&format!("{h} ")) || g.starts_with(&format!("{h}!")))
    {
        return "Hey! ICE here. Ask me anything, or tell me what to build.".into();
    }
    if g.contains("thank") {
        return "Anytime. What's next?".into();
    }
    format!(
        "(demo mode) I'd normally answer that with a live model. Set a provider key with /onboard to chat for real.\nYou said: {}",
        goal.trim()
    )
}

fn run_ice_turns(
    job: &Job,
    send: &dyn Fn(Event),
    goal: &str,
    mut last_delta: Option<String>,
    max_turns: u32,
    loop_pass: Option<u32>,
) -> Result<bool> {
    let mut turns = 0u32;
    loop {
        turns += 1;
        if turns > max_turns {
            return Ok(false);
        }
        send(Event::Thinking);
        let raw = if job.demo {
            send(Event::ThinkingText(
                "demo compiler: pick a minimal burst that can close the goal.".into(),
            ));
            demo_burst(goal, turns, last_delta.as_deref(), loop_pass)
        } else {
            let llm = Llm::from_env().ok_or_else(|| {
                anyhow::anyhow!("no API key. Run /onboard or set a provider key.")
            })?;
            let user = build_user(goal, last_delta.as_deref(), loop_pass);
            let comp = llm.complete(llm::SYSTEM_PROMPT, &user)?;
            if !comp.thinking.trim().is_empty() {
                send(Event::ThinkingText(comp.thinking.clone()));
            }
            send(Event::Tokens {
                input: crate::theme::estimate_tokens(&user)
                    + crate::theme::estimate_tokens(llm::SYSTEM_PROMPT),
                output: crate::theme::estimate_tokens(&comp.content)
                    + crate::theme::estimate_tokens(&comp.thinking),
            });
            comp.content
        };
        send(Event::BurstSource(raw.clone()));

        let burst = Burst::parse(&raw)?;
        let mut steps: Vec<StepResult> = Vec::new();
        let mut yielded = false;
        for action in &burst.actions {
            match action {
                Action::Yield { .. } => {
                    yielded = true;
                    let step = exec::run_action(&job.root, action);
                    send(Event::Step(step.clone()));
                    steps.push(step);
                }
                Action::Agent { name, goal: ag } => {
                    send(Event::AgentStart {
                        name: name.clone(),
                        goal: ag.clone(),
                    });
                    let step = agents::run_subagent(&job.root, name, ag, job.demo, 2);
                    send(Event::AgentDone {
                        name: name.clone(),
                        ok: step.ok,
                        summary: step.output.clone(),
                    });
                    send(Event::Step(step.clone()));
                    steps.push(step);
                }
                _ => {
                    let step = exec::run_action(&job.root, action);
                    send(Event::Step(step.clone()));
                    steps.push(step);
                }
            }
        }
        let asserts = verify::check(&job.root, &burst.asserts, &steps);
        send(Event::Asserts(
            asserts
                .iter()
                .map(|a| (a.label.clone(), a.ok, a.detail.clone()))
                .collect(),
        ));
        let packed = delta::pack(&steps, &asserts);
        send(Event::Delta(packed.clone()));
        let _ = DurableGoal::append_progress(
            &job.root,
            &format!("### burst {turns}\n{}\n", packed.summary),
        );

        if packed.ok {
            send(Event::Assistant(format!(
                "Burst {} closed this inner pass.\n{}",
                turns, packed.summary
            )));
            if loop_pass.is_none() {
                send(Event::Done { turns, ok: true });
            }
            return Ok(true);
        }
        if yielded {
            send(Event::Assistant(
                "Burst yielded for a decision. Reply to continue.".into(),
            ));
            if loop_pass.is_none() {
                send(Event::Done { turns, ok: false });
            }
            return Ok(false);
        }
        last_delta = Some(packed.summary);
    }
}

fn build_user(goal: &str, delta: Option<&str>, loop_pass: Option<u32>) -> String {
    let header = match loop_pass {
        Some(p) => format!("Outer loop pass {p}. Context is the goal + progress only.\n"),
        None => String::new(),
    };
    match delta {
        None => format!("{header}Workspace task:\n{goal}\n\nEmit the first BURST."),
        Some(d) => format!(
            "{header}Original goal:\n{goal}\n\nPrevious DELTA / progress:\n{d}\n\nEmit the next BURST only."
        ),
    }
}

fn demo_burst(goal: &str, turn: u32, delta: Option<&str>, loop_pass: Option<u32>) -> String {
    if loop_pass.is_some() {
        if turn == 1 && delta.is_none() {
            return format!(
                "GOAL: {goal}\nBURST:\n  agent scout: map workspace files\n  agent writer: touch .ice/DONE and record a note\n  list .ice\nASSERT:\n  file_exists .ice/DONE\n"
            );
        }
        return format!(
            "GOAL: {goal}\nBURST:\n  write .ice/DONE\n  <<\nclosed\n  >>\n  run echo goal marker written\nASSERT:\n  file_exists .ice/DONE\n"
        );
    }
    if turn == 1 && delta.is_none() {
        format!("GOAL: {goal}\nBURST:\n  list .\n  run pwd\n  agent scout: list src if present\nASSERT:\n  exit 0\n")
    } else {
        format!("GOAL: {goal}\nBURST:\n  run echo ICE demo complete\nASSERT:\n  exit 0\n")
    }
}
