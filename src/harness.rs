//! Agentic loop — the Claude-Code-style core, on a fast Rust harness.
//!
//! The model works step by step: each turn it calls one or more tools, we run
//! them and feed the results back, and it continues until it answers with no
//! tool call. There is no "burst schema / ASSERT / DONE-marker" ceremony — the
//! model decides when the task is finished by simply stopping.

use crate::agents;
use crate::exec::{self, StepResult};
use crate::goal::DurableGoal;
use crate::ir::{self, Action};
use crate::llm::{self, Llm};
use crate::sandbox::clip;
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
    Delta(crate::delta::Delta),
    Assistant(String),
    Loop { pass: u32, max: u32, note: String },
    AgentStart { name: String, goal: String },
    AgentDone { name: String, ok: bool, summary: String },
    Goal { text: String, done: bool },
    Done { turns: u32, ok: bool },
    Error(String),
    Tokens { input: u32, output: u32 },
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
    // A selected CLI agent (Codex/ChatGPT, Claude Code, opencode…) is the
    // backend: hand the whole message to it and show its output.
    if let Some(agent) = crate::providers::selected_cli_agent() {
        return run_cli_backend(job, send, agent);
    }
    if job.chat_mode {
        return run_chat(job, send);
    }
    // A goal gets a larger step budget; a plain task a smaller one.
    let max_steps = if job.loop_mode {
        job.max_turns.max(40)
    } else {
        job.max_turns.max(16)
    };
    if job.loop_mode {
        send(Event::Goal {
            text: job.goal.clone(),
            done: false,
        });
    }
    run_agent(job, send, max_steps)
}

/// The agentic loop.
fn run_agent(job: &Job, send: &dyn Fn(Event), max_steps: u32) -> Result<()> {
    if job.demo {
        send(Event::Assistant(
            "Demo mode: no model connected. Run /onboard to set a provider key, then try again."
                .into(),
        ));
        send(Event::Done { turns: 1, ok: true });
        return Ok(());
    }
    let llm =
        Llm::from_env().ok_or_else(|| anyhow::anyhow!("no API key. Run /onboard or set a key."))?;

    let mut messages: Vec<(String, String)> = vec![("user".into(), job.goal.clone())];
    let mut last_sig = String::new();
    let mut repeats = 0u32;

    for step in 1..=max_steps {
        send(Event::Thinking);
        if job.loop_mode {
            send(Event::Loop {
                pass: step,
                max: max_steps,
                note: format!("step {step}/{max_steps}"),
            });
        }
        let comp = llm.complete_messages(llm::SYSTEM_PROMPT, &messages)?;
        if !comp.thinking.trim().is_empty() {
            send(Event::ThinkingText(comp.thinking.clone()));
        }
        send(Event::Tokens {
            input: transcript_tokens(&messages) + crate::theme::estimate_tokens(llm::SYSTEM_PROMPT),
            output: crate::theme::estimate_tokens(&comp.content)
                + crate::theme::estimate_tokens(&comp.thinking),
        });

        let parsed = ir::parse_agent_step(&comp.content);
        messages.push(("assistant".into(), comp.content.clone()));

        // No tool calls ⇒ final answer, task complete.
        if parsed.actions.is_empty() {
            let final_text = if parsed.text.trim().is_empty() {
                comp.content.trim().to_string()
            } else {
                parsed.text
            };
            if !final_text.trim().is_empty() {
                send(Event::Assistant(final_text));
            }
            send(Event::Done {
                turns: step,
                ok: true,
            });
            return Ok(());
        }

        // Any narration before the tool calls.
        if !parsed.text.trim().is_empty() {
            send(Event::Assistant(parsed.text.clone()));
        }

        // Run the tools and gather results for the next turn.
        let mut results = String::new();
        for action in &parsed.actions {
            match action {
                Action::Agent { name, goal } => {
                    send(Event::AgentStart {
                        name: name.clone(),
                        goal: goal.clone(),
                    });
                    let s = agents::run_subagent(&job.root, name, goal, job.demo, 3);
                    send(Event::AgentDone {
                        name: name.clone(),
                        ok: s.ok,
                        summary: s.output.clone(),
                    });
                    results.push_str(&format!(
                        "\n$ agent {name} → {}\n{}\n",
                        if s.ok { "ok" } else { "fail" },
                        clip(&s.output, 1500)
                    ));
                }
                _ => {
                    let s = exec::run_action(&job.root, action);
                    send(Event::Step(s.clone()));
                    send(Event::TodoChanged);
                    results.push_str(&format!(
                        "\n$ {} → exit {}\n{}\n",
                        s.title,
                        s.exit,
                        clip(&s.output, 1500)
                    ));
                }
            }
        }
        messages.push((
            "user".into(),
            format!(
                "Tool results:\n{}\nContinue with the next step, or reply with the final answer if the task is complete.",
                results.trim()
            ),
        ));

        // Break out of an unproductive loop repeating the same actions.
        let sig = action_signature(&parsed.actions);
        if sig == last_sig {
            repeats += 1;
            if repeats >= 3 {
                send(Event::Assistant(
                    "Stopping — I'm repeating the same step without progress. Please refine the request or switch model (/models).".into(),
                ));
                send(Event::Done {
                    turns: step,
                    ok: false,
                });
                return Ok(());
            }
        } else {
            repeats = 0;
        }
        last_sig = sig;

        // Keep the transcript bounded so long tasks don't blow the context.
        trim_messages(&mut messages, 24);
    }

    send(Event::Assistant(format!(
        "Reached the step budget ({max_steps}). The task may be partially done — send another message to continue."
    )));
    send(Event::Done {
        turns: max_steps,
        ok: false,
    });
    Ok(())
}

fn action_signature(actions: &[Action]) -> String {
    actions
        .iter()
        .map(|a| match a {
            Action::Read { path } => format!("read:{path}"),
            Action::List { path } => format!("list:{path}"),
            Action::Grep { pattern, path } => format!("grep:{pattern}:{path}"),
            Action::Write { path, .. } => format!("write:{path}"),
            Action::Replace { path, .. } => format!("replace:{path}"),
            Action::Run { cmd } => format!("run:{cmd}"),
            Action::WebSearch { query } => format!("web:{query}"),
            Action::WebFetch { url } => format!("fetch:{url}"),
            Action::Mcp { tool, .. } => format!("mcp:{tool}"),
            Action::Skill { name } => format!("skill:{name}"),
            Action::Agent { name, .. } => format!("agent:{name}"),
            Action::TodoAdd { text } => format!("todo:{text}"),
            Action::TodoDone { key } => format!("tododone:{key}"),
            Action::Yield { reason } => format!("yield:{reason}"),
        })
        .collect::<Vec<_>>()
        .join("|")
}

fn transcript_tokens(msgs: &[(String, String)]) -> u32 {
    msgs.iter()
        .map(|(_, c)| crate::theme::estimate_tokens(c))
        .sum()
}

/// Keep the system-less transcript to the last `keep` messages, preserving the
/// original task as the first entry.
fn trim_messages(msgs: &mut Vec<(String, String)>, keep: usize) {
    if msgs.len() <= keep {
        return;
    }
    let first = msgs[0].clone();
    let tail_start = msgs.len() - (keep - 1);
    let mut trimmed = vec![first];
    trimmed.extend_from_slice(&msgs[tail_start..]);
    *msgs = trimmed;
}

/// Delegate the message to a logged-in CLI agent and surface its output.
fn run_cli_backend(
    job: &Job,
    send: &dyn Fn(Event),
    agent: &crate::providers::CliAgent,
) -> Result<()> {
    if !crate::providers::cli_agent_present(agent) {
        send(Event::Assistant(format!(
            "`{}` isn't on PATH. Install it, then: {}",
            agent.bin, agent.login_hint
        )));
        send(Event::Done { turns: 1, ok: false });
        return Ok(());
    }
    send(Event::Thinking);
    send(Event::Step(StepResult {
        title: format!("agent {}", agent.bin),
        ok: true,
        exit: 0,
        output: format!("delegating to {}…", agent.name),
        ms: 0,
    }));
    match crate::cli::complete(agent, &job.goal, &job.root) {
        Ok(text) => {
            send(Event::Assistant(if text.trim().is_empty() {
                "(no output)".into()
            } else {
                text
            }));
            send(Event::Done { turns: 1, ok: true });
        }
        Err(e) => {
            send(Event::Error(e.to_string()));
            send(Event::Done { turns: 1, ok: false });
        }
    }
    Ok(())
}

/// Fast conversational path: one completion, plain text, no tools.
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
