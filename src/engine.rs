//! The agent loop. One `Engine` owns a conversation: it streams a model
//! turn, checks permissions, runs tools (read-only ones in parallel), feeds
//! results back, and repeats until the model answers without tools —
//! with interruption, auto-compaction, hooks, sub-agents and transcripts.

use crate::agents;
use crate::conv::{Block, Message, Role, Usage};
use crate::hooks::Hooks;
use crate::http;
use crate::mcp;
use crate::model::{self, ModelConfig, StreamEvent, ToolSpec, TurnRequest};
use crate::permissions::{self, Checker, Decision, Mode, Rules};
use crate::prompt;
use crate::store::{self, Transcript};
use crate::tools::{self, DiffLine, Display, TodoItem, ToolCtx, ToolOutput};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const REJECT_MESSAGE: &str = "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). STOP what you are doing and wait for the user to tell you how to proceed.";
pub const INTERRUPT_MESSAGE: &str = "[Request interrupted by user]";
pub const INTERRUPT_TOOL_MESSAGE: &str = "[Request interrupted by user for tool use]";

/// What the permission prompt shows.
#[derive(Clone, Debug)]
pub enum PermDetail {
    Command {
        command: String,
        description: String,
    },
    Diff {
        title: String,
        lines: Vec<DiffLine>,
    },
    Plan(String),
    Fetch {
        url: String,
    },
    Generic {
        summary: String,
    },
}

#[derive(Clone, Debug)]
pub enum PermReply {
    Allow,
    /// "Yes, and don't ask again" (the rule text shown in the prompt).
    AllowAlways,
    Deny(Option<String>),
}

#[derive(Clone, Debug)]
pub struct PermRequest {
    pub tool: String,
    pub detail: PermDetail,
    /// Label for the "don't ask again" option, if the tool supports one.
    pub always_label: Option<String>,
    pub reply: Sender<PermReply>,
}

#[derive(Clone, Debug)]
pub enum Event {
    /// A model request is starting (spinner on).
    Requesting,
    TextDelta(String),
    ThinkingDelta(String),
    /// A finished assistant text block.
    Text(String),
    Thinking(String),
    ToolStart {
        id: String,
        name: String,
        input: Value,
    },
    ToolDone {
        id: String,
        name: String,
        input: Value,
        output: ToolOutput,
    },
    /// Progress inside a Task sub-agent: (task tool id, one-line activity).
    SubagentStep {
        parent: String,
        line: String,
    },
    Permission(PermRequest),
    Retry {
        attempt: u32,
        error: String,
        wait: Duration,
    },
    Usage {
        turn: Usage,
        total: Usage,
        context: u64,
        window: u64,
        cost: Option<f64>,
    },
    Todos(Vec<TodoItem>),
    ModeChanged(Mode),
    Compacted {
        auto: bool,
    },
    Notice(String),
    Error(String),
    Done(TurnResult),
    /// Worker-level events (not produced inside a turn).
    Ready(Box<Ready>),
    Stats(Stats),
    /// Plain command output for the transcript (slash commands).
    Info(String),
    /// A session was resumed: its messages, for re-rendering.
    Resumed {
        id: String,
        messages: Vec<Message>,
    },
    BashDone {
        command: String,
        output: ToolOutput,
    },
    Cleared,
    Rewound,
}

pub struct Ready {
    pub ctx: Arc<ToolCtx>,
    pub mcp: Arc<mcp::Pool>,
    pub stats: Stats,
}

impl std::fmt::Debug for Ready {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Ready")
    }
}

impl Clone for Ready {
    fn clone(&self) -> Self {
        Ready {
            ctx: self.ctx.clone(),
            mcp: self.mcp.clone(),
            stats: self.stats.clone(),
        }
    }
}

/// A snapshot of engine state for the status line and /status, /cost.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub session_id: String,
    pub model: String,
    pub base_url: String,
    pub configured: bool,
    pub mode: Option<Mode>,
    pub usage: Usage,
    pub cost: Option<f64>,
    pub api_ms: u128,
    pub wall_ms: u128,
    pub lines_added: usize,
    pub lines_removed: usize,
    pub context: u64,
    pub window: u64,
    pub messages: usize,
    pub text_tools: bool,
}

#[derive(Clone, Debug, Default)]
pub struct TurnResult {
    pub text: String,
    pub is_error: bool,
    pub interrupted: bool,
    pub num_turns: u32,
    pub duration_ms: u128,
}

pub type Emit<'a> = &'a (dyn Fn(Event) + Sync);

pub struct Options {
    pub root: PathBuf,
    pub model_override: Option<String>,
    pub mode: Mode,
    pub interactive: bool,
    pub max_turns: Option<u32>,
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    pub append_system_prompt: Option<String>,
    pub system_prompt: Option<String>,
    pub session_id: Option<String>,
    pub connect_mcp: bool,
    pub cancel: Arc<AtomicBool>,
}

pub struct Engine {
    pub root: PathBuf,
    pub cfg: Option<ModelConfig>,
    pub messages: Vec<Message>,
    pub ctx: Arc<ToolCtx>,
    pub mcp: Arc<mcp::Pool>,
    pub hooks: Hooks,
    pub mode: Mode,
    pub rules: Rules,
    pub interactive: bool,
    pub max_turns: Option<u32>,
    pub usage: Usage,
    pub cost: f64,
    pub cost_known: bool,
    pub last_context: u64,
    pub transcript: Transcript,
    pub session_id: String,
    pub text_tools: bool,
    pub auto_compact: bool,
    /// Session goal from /goal, restated in the system prompt every turn.
    pub goal: Option<String>,
    append_system: Option<String>,
    replace_system: Option<String>,
    git_status: Option<String>,
    subagent: Option<agents::AgentDef>,
    pub api_ms: u128,
    pub started: Instant,
    pub lines_added: usize,
    pub lines_removed: usize,
    /// Usage from sub-agents, merged into the totals after tools finish.
    pending_usage: std::sync::Mutex<Usage>,
    /// (message index, block index) where each user prompt starts — for rewind.
    prompt_marks: Vec<(usize, usize)>,
}

impl Engine {
    pub fn new(o: Options) -> Engine {
        let settings = crate::settings::Settings::load(&o.root);
        let model_override = o.model_override.clone().or(settings.model.clone());
        let cfg = ModelConfig::resolve(model_override.as_deref());
        let mut rules = Rules {
            allow: settings.allow.clone(),
            deny: settings.deny.clone(),
            ask: settings.ask.clone(),
        };
        rules
            .allow
            .extend(o.allowed_tools.iter().flat_map(|s| split_tool_list(s)));
        rules
            .deny
            .extend(o.disallowed_tools.iter().flat_map(|s| split_tool_list(s)));
        let session_id = o.session_id.clone().unwrap_or_else(crate::conv::uuid);
        let mcp = if o.connect_mcp {
            mcp::Pool::connect(&o.root)
        } else {
            mcp::Pool::empty(&o.root)
        };
        Engine {
            ctx: Arc::new(ToolCtx::new(o.root.clone(), o.cancel.clone())),
            mcp: Arc::new(mcp),
            hooks: Hooks::load(&o.root),
            transcript: Transcript::new(&o.root, &session_id),
            session_id,
            git_status: prompt::git_status(&o.root),
            root: o.root,
            cfg,
            messages: Vec::new(),
            mode: o.mode,
            rules,
            interactive: o.interactive,
            max_turns: o.max_turns,
            usage: Usage::default(),
            cost: 0.0,
            cost_known: true,
            last_context: 0,
            text_tools: std::env::var_os("ICE_TEXT_TOOLS").is_some(),
            auto_compact: settings.auto_compact,
            goal: None,
            append_system: o.append_system_prompt,
            replace_system: o.system_prompt,
            subagent: None,
            api_ms: 0,
            started: Instant::now(),
            lines_added: 0,
            lines_removed: 0,
            pending_usage: std::sync::Mutex::new(Usage::default()),
            prompt_marks: Vec::new(),
        }
    }

    pub fn stats(&self) -> Stats {
        Stats {
            session_id: self.session_id.clone(),
            model: self.model_name(),
            base_url: self
                .cfg
                .as_ref()
                .map(|c| c.base_url.clone())
                .unwrap_or_default(),
            configured: self.cfg.is_some() || crate::providers::selected_cli_agent().is_some(),
            mode: Some(self.mode),
            usage: self.usage,
            cost: if self.cost_known {
                Some(self.cost)
            } else {
                None
            },
            api_ms: self.api_ms,
            wall_ms: self.started.elapsed().as_millis(),
            lines_added: self.lines_added,
            lines_removed: self.lines_removed,
            context: self.last_context,
            window: self.window(),
            messages: self.messages.len(),
            text_tools: self.text_tools,
        }
    }

    /// Drop the conversation back to just before user prompt `k` (0-based).
    /// The transcript continues in a fresh file so resume stays consistent.
    pub fn rewind(&mut self, k: usize) -> bool {
        let Some(&(mi, bi)) = self.prompt_marks.get(k) else {
            return false;
        };
        self.messages.truncate(mi + 1);
        if let Some(m) = self.messages.get_mut(mi) {
            m.content.truncate(bi);
            if m.content.is_empty() {
                self.messages.pop();
            }
        }
        self.prompt_marks.truncate(k);
        store::repair(&mut self.messages);
        self.session_id = crate::conv::uuid();
        self.transcript = Transcript::new(&self.root, &self.session_id);
        for m in &self.messages {
            self.transcript.message(m);
        }
        self.last_context = self.messages.iter().map(|m| m.approx_tokens()).sum();
        true
    }

    /// Approximate context breakdown for /context.
    pub fn context_breakdown(&self) -> Vec<(String, u64)> {
        let sys = (self.system_prompt().len() / 4) as u64;
        let tools: u64 = self
            .tool_specs()
            .iter()
            .map(|t| ((t.description.len() + t.input_schema.to_string().len()) / 4) as u64)
            .sum();
        let mcp: u64 = self
            .mcp
            .specs()
            .iter()
            .map(|t| ((t.description.len() + t.input_schema.to_string().len()) / 4) as u64)
            .sum();
        let memory: u64 = prompt::load_memory(&self.root)
            .iter()
            .map(|m| (m.content.len() / 4) as u64)
            .sum();
        let msgs: u64 = self.messages.iter().map(|m| m.approx_tokens()).sum();
        vec![
            ("System prompt".into(), sys.saturating_sub(memory)),
            ("System tools".into(), tools.saturating_sub(mcp)),
            ("MCP tools".into(), mcp),
            ("Memory files".into(), memory),
            ("Messages".into(), msgs),
        ]
    }

    pub fn model_name(&self) -> String {
        self.cfg
            .as_ref()
            .map(|c| c.model.clone())
            .unwrap_or_else(|| "(not configured)".into())
    }

    pub fn window(&self) -> u64 {
        self.cfg
            .as_ref()
            .map(|c| c.context_window())
            .unwrap_or(200_000)
    }

    pub fn set_model(&mut self, m: &str) {
        self.cfg = ModelConfig::resolve(Some(m));
    }

    pub fn reload_model(&mut self) {
        let m = self.cfg.as_ref().map(|c| c.model.clone());
        self.cfg = ModelConfig::resolve(m.as_deref());
    }

    fn tool_specs(&self) -> Vec<ToolSpec> {
        let skills = crate::skills::listing(&self.root);
        let mut specs = tools::builtin_specs(&skills);
        if let Some(t) = specs.iter_mut().find(|s| s.name == "Task") {
            t.description = format!(
                "{}\n\nAvailable agent types and the tools they have access to:\n{}",
                t.description,
                agents::listing(&self.root)
            );
        }
        specs.extend(self.mcp.specs());
        if let Some(a) = &self.subagent {
            specs.retain(|s| s.name != "Task" && s.name != "ExitPlanMode");
            if let Some(allowed) = &a.tools {
                specs.retain(|s| {
                    allowed.iter().any(|t| {
                        t == &s.name || (t.starts_with("mcp__") && s.name.starts_with(t.as_str()))
                    })
                });
            }
        } else if self.mode != Mode::Plan {
            specs.retain(|s| s.name != "ExitPlanMode");
        }
        specs
    }

    pub fn system_prompt(&self) -> String {
        let memory = if self.subagent.is_some() {
            Vec::new()
        } else {
            prompt::load_memory(&self.root)
        };
        let role = self.subagent.as_ref().map(|a| a.prompt.clone());
        let goal_text = self.goal.as_ref().filter(|_| self.subagent.is_none()).map(|g| {
            let mut t = format!("# Session goal\nThe user set this goal for the session. Keep working toward it and say when it is achieved:\n{g}");
            if let Some(a) = &self.append_system {
                t = format!("{a}\n\n{t}");
            }
            t
        });
        prompt::system_prompt(&prompt::PromptParts {
            root: &self.root,
            model: &self.model_name(),
            memory: &memory,
            git_status: if self.subagent.is_some() {
                None
            } else {
                self.git_status.as_deref()
            },
            append: goal_text.as_deref().or(self.append_system.as_deref()),
            replace: self.replace_system.as_deref(),
            subagent: role.as_deref(),
        })
    }

    fn record(&mut self, m: Message) {
        self.transcript.message(&m);
        // Keep strict user/assistant alternation for the APIs.
        match self.messages.last_mut() {
            Some(last) if last.role == m.role => last.content.extend(m.content),
            _ => self.messages.push(m),
        }
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.prompt_marks.clear();
        self.last_context = 0;
        self.session_id = crate::conv::uuid();
        self.transcript = Transcript::new(&self.root, &self.session_id);
        *self.ctx.todos.lock().unwrap() = Vec::new();
        self.ctx.read_state.lock().unwrap().clear();
    }

    /// Resume a stored session into this engine.
    pub fn resume(&mut self, id_or_path: &str) -> Result<String> {
        let path = if id_or_path.ends_with(".jsonl") {
            PathBuf::from(id_or_path)
        } else {
            store::project_dir(&self.root).join(format!("{id_or_path}.jsonl"))
        };
        if !path.exists() {
            return Err(anyhow!(
                "No conversation found with session ID: {id_or_path}"
            ));
        }
        let (msgs, usage) = store::load(&path)?;
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        self.prompt_marks = msgs
            .iter()
            .enumerate()
            .filter(|(_, m)| m.role == Role::User)
            .filter_map(|(i, m)| {
                m.content
                    .iter()
                    .position(|b| matches!(b, Block::Text { text } if is_user_prompt(text)))
                    .map(|bi| (i, bi))
            })
            .collect();
        self.messages = msgs;
        self.usage = usage;
        self.session_id = id.clone();
        self.transcript = Transcript::new(&self.root, &id);
        Ok(id)
    }

    fn emit_usage(&mut self, turn: Usage, emit: Emit) {
        self.usage.add(&turn);
        self.last_context = turn.context_tokens();
        let model = self.model_name();
        match model::cost(&model, &turn) {
            Some(c) => self.cost += c,
            None => self.cost_known = false,
        }
        self.transcript.usage(&model, &turn);
        emit(Event::Usage {
            turn,
            total: self.usage,
            context: self.last_context,
            window: self.window(),
            cost: if self.cost_known {
                Some(self.cost)
            } else {
                None
            },
        });
    }

    /// Sub-agent usage counts toward cost and totals but not this context.
    fn add_subagent_usage(&mut self, u: Usage, emit: Emit) {
        self.usage.add(&u);
        match model::cost(&self.model_name(), &u) {
            Some(c) => self.cost += c,
            None => self.cost_known = false,
        }
        emit(Event::Usage {
            turn: Usage::default(),
            total: self.usage,
            context: self.last_context,
            window: self.window(),
            cost: if self.cost_known {
                Some(self.cost)
            } else {
                None
            },
        });
    }

    /// `@path` mentions attach file contents (and mark them read).
    fn attachments(&self, prompt: &str) -> Vec<Block> {
        let re = regex::Regex::new(r#"(?:^|\s)@("[^"]+"|[^\s]+)"#).unwrap();
        let mut out = Vec::new();
        for c in re.captures_iter(prompt) {
            let raw = c[1]
                .trim_matches('"')
                .trim_end_matches([',', '.', ')', ';', ':']);
            let p = self.ctx.resolve(raw);
            if p.is_file() {
                let r = tools::run(
                    &self.ctx,
                    "Read",
                    &json!({"file_path": p.display().to_string()}),
                );
                if !r.is_error {
                    out.push(Block::Text {
                        text: format!(
                            "<system-reminder>\nCalled the Read tool with the following input: {}\nResult of calling the Read tool:\n{}\n</system-reminder>",
                            json!({"file_path": p.display().to_string()}),
                            r.content
                        ),
                    });
                }
            } else if p.is_dir() {
                let r = tools::run(&self.ctx, "LS", &json!({"path": p.display().to_string()}));
                out.push(Block::Text {
                    text: format!("<system-reminder>\nCalled the LS tool with the following input: {}\nResult of calling the LS tool:\n{}\n</system-reminder>", json!({"path": p.display().to_string()}), r.content),
                });
            }
        }
        out
    }

    /// `!command` from the prompt: run it directly and add the exchange to
    /// the conversation so the model sees it next turn.
    pub fn user_bash(&mut self, command: &str) -> ToolOutput {
        let out = tools::run(&self.ctx, "Bash", &json!({"command": command}));
        let (stdout, stderr) = if out.is_error {
            (String::new(), out.content.clone())
        } else {
            (out.content.clone(), String::new())
        };
        self.record(Message {
            role: Role::User,
            content: vec![
                Block::Text {
                    text: format!("<bash-input>{command}</bash-input>"),
                },
                Block::Text {
                    text: format!(
                        "<bash-stdout>{stdout}</bash-stdout><bash-stderr>{stderr}</bash-stderr>"
                    ),
                },
            ],
        });
        out
    }

    /// Run one user prompt to completion.
    pub fn submit(&mut self, prompt_text: &str, emit: Emit) -> TurnResult {
        let t0 = Instant::now();
        self.ctx.cancel.store(false, Ordering::Relaxed);
        let r = self.submit_inner(prompt_text, emit);
        let mut result = match r {
            Ok(r) => r,
            Err(e) if http::is_interrupted(&e) => self.interrupted(emit),
            Err(e) => {
                let msg = e.to_string();
                emit(Event::Error(msg.clone()));
                TurnResult {
                    text: msg,
                    is_error: true,
                    ..Default::default()
                }
            }
        };
        result.duration_ms = t0.elapsed().as_millis();
        emit(Event::Done(result.clone()));
        result
    }

    fn interrupted(&mut self, _emit: Emit) -> TurnResult {
        self.record(Message::user_text(INTERRUPT_MESSAGE));
        store::repair(&mut self.messages);
        TurnResult {
            text: INTERRUPT_MESSAGE.into(),
            is_error: true,
            interrupted: true,
            ..Default::default()
        }
    }

    fn submit_inner(&mut self, prompt_text: &str, emit: Emit) -> Result<TurnResult> {
        // UserPromptSubmit hooks can block or add context.
        let mut extra_context = Vec::new();
        if !self.hooks.is_empty() {
            let o = self.hooks.run(
                &self.root,
                "UserPromptSubmit",
                None,
                json!({"prompt": prompt_text, "session_id": self.session_id}),
            );
            for n in o.notes {
                emit(Event::Notice(n));
            }
            if let Some(reason) = o.block {
                emit(Event::Error(format!("Prompt blocked by hook: {reason}")));
                return Ok(TurnResult {
                    text: reason,
                    is_error: true,
                    ..Default::default()
                });
            }
            extra_context = o.context;
        }

        if let Some(agent) = crate::providers::selected_cli_agent() {
            return self.run_cli_agent(agent, prompt_text, emit);
        }
        let Some(cfg) = self.cfg.clone() else {
            let msg = "No model is configured. Run /login to choose a provider and paste an API key, or set one of: ANTHROPIC_API_KEY, OPENAI_API_KEY, GEMINI_API_KEY, XAI_API_KEY, GROQ_API_KEY, OPENROUTER_API_KEY (then restart).";
            emit(Event::Error(msg.into()));
            return Ok(TurnResult {
                text: msg.into(),
                is_error: true,
                ..Default::default()
            });
        };

        let mut content = vec![Block::Text {
            text: prompt_text.to_string(),
        }];
        content.extend(self.attachments(prompt_text));
        for c in extra_context {
            content.push(Block::Text {
                text: format!("<system-reminder>\n{c}\n</system-reminder>"),
            });
        }
        if self.mode == Mode::Plan && self.subagent.is_none() {
            content.push(Block::Text {
                text: prompt::PLAN_MODE_REMINDER.into(),
            });
        }
        let mark = match self.messages.last() {
            Some(m) if m.role == Role::User => (self.messages.len() - 1, m.content.len()),
            _ => (self.messages.len(), 0),
        };
        self.prompt_marks.push(mark);
        self.record(Message {
            role: Role::User,
            content,
        });
        let thinking = prompt::thinking_budget(prompt_text);
        self.run_loop(&cfg, thinking, emit)
    }

    fn run_loop(
        &mut self,
        cfg: &ModelConfig,
        thinking_budget: u32,
        emit: Emit,
    ) -> Result<TurnResult> {
        let mut turns = 0u32;
        let mut final_text = String::new();
        loop {
            if self.ctx.cancelled() {
                return Err(http::Interrupted.into());
            }
            if let Some(max) = self.max_turns {
                if turns >= max {
                    let msg = format!("Reached max turns ({max})");
                    emit(Event::Error(msg.clone()));
                    return Ok(TurnResult {
                        text: msg,
                        is_error: true,
                        num_turns: turns,
                        ..Default::default()
                    });
                }
            }
            // Auto-compact before the context overflows.
            if self.auto_compact
                && self.subagent.is_none()
                && self.last_context > 0
                && self.last_context as f64 > self.window() as f64 * 0.92
            {
                emit(Event::Notice(
                    "Context low · auto-compacting conversation…".into(),
                ));
                self.compact(None, emit)?;
                emit(Event::Compacted { auto: true });
            }
            turns += 1;
            emit(Event::Requesting);
            let specs = self.tool_specs();
            let mut system = self.system_prompt();
            if self.text_tools {
                system.push_str(&model::text_protocol::instructions(&specs));
            }
            let api_t0 = Instant::now();
            let partial = std::sync::Mutex::new(String::new());
            let resp = {
                let mut on = |ev: StreamEvent| match ev {
                    StreamEvent::Text(t) => {
                        partial.lock().unwrap().push_str(&t);
                        emit(Event::TextDelta(t));
                    }
                    StreamEvent::Thinking(t) => emit(Event::ThinkingDelta(t)),
                    StreamEvent::ToolStart => {}
                    StreamEvent::Retry {
                        attempt,
                        error,
                        wait,
                    } => emit(Event::Retry {
                        attempt,
                        error,
                        wait,
                    }),
                };
                let req = TurnRequest {
                    system: &system,
                    messages: &self.messages,
                    tools: &specs,
                    thinking_budget,
                    text_tools: self.text_tools,
                };
                match model::stream_turn(cfg, &req, &self.ctx.cancel, &mut on) {
                    Err(e) if !self.text_tools && model::is_tools_unsupported(&e) => {
                        // Fall back to the text tool protocol for this model.
                        self.text_tools = true;
                        emit(Event::Notice(format!(
                            "{} has no native tool calling · using ICE's text tool protocol",
                            cfg.model
                        )));
                        system.push_str(&model::text_protocol::instructions(&specs));
                        let req = TurnRequest {
                            system: &system,
                            messages: &self.messages,
                            tools: &specs,
                            thinking_budget,
                            text_tools: true,
                        };
                        model::stream_turn(cfg, &req, &self.ctx.cancel, &mut on)
                    }
                    other => other,
                }
            };
            self.api_ms += api_t0.elapsed().as_millis();
            let resp = match resp {
                Ok(r) => r,
                Err(e) => {
                    if http::is_interrupted(&e) {
                        let p = partial.into_inner().unwrap();
                        if !p.trim().is_empty() {
                            self.record(Message::assistant_text(p));
                        }
                    }
                    return Err(e);
                }
            };
            self.emit_usage(resp.usage, emit);

            let content = resp.content.clone();
            for b in &content {
                match b {
                    Block::Text { text } if !text.trim().is_empty() => {
                        emit(Event::Text(text.clone()))
                    }
                    Block::Thinking { thinking, .. } if !thinking.trim().is_empty() => {
                        emit(Event::Thinking(thinking.clone()))
                    }
                    _ => {}
                }
            }
            let text_now: String = content
                .iter()
                .filter_map(|b| {
                    if let Block::Text { text } = b {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            if !text_now.trim().is_empty() {
                final_text = text_now;
            }
            if content.is_empty() {
                // Nothing to add; an empty assistant turn is invalid history.
                break;
            }
            self.record(Message {
                role: Role::Assistant,
                content: content.clone(),
            });

            let calls: Vec<(String, String, Value)> = content
                .iter()
                .filter_map(|b| {
                    if let Block::ToolUse { id, name, input } = b {
                        Some((id.clone(), name.clone(), input.clone()))
                    } else {
                        None
                    }
                })
                .collect();
            if calls.is_empty() {
                if resp.stop_reason == "max_tokens" {
                    emit(Event::Notice("Response hit the output token limit (set ICE_MAX_OUTPUT_TOKENS to raise it).".into()));
                }
                // Stop hooks may send the model back to work.
                if !self.hooks.is_empty() && self.subagent.is_none() {
                    let o = self.hooks.run(
                        &self.root,
                        "Stop",
                        None,
                        json!({"session_id": self.session_id, "stop_hook_active": false}),
                    );
                    if let Some(reason) = o.block {
                        self.record(Message::user_text(format!("Stop hook feedback:\n{reason}")));
                        continue;
                    }
                }
                break;
            }

            let (results, stop) = self.run_tools(&calls, &specs, emit);
            let sub = std::mem::take(&mut *self.pending_usage.lock().unwrap());
            if sub != Usage::default() {
                self.add_subagent_usage(sub, emit);
            }
            self.record(Message {
                role: Role::User,
                content: results,
            });
            if stop {
                return Ok(TurnResult {
                    text: final_text,
                    is_error: false,
                    num_turns: turns,
                    ..Default::default()
                });
            }
            if self.ctx.cancelled() {
                return Err(http::Interrupted.into());
            }
        }
        Ok(TurnResult {
            text: final_text,
            is_error: false,
            num_turns: turns,
            ..Default::default()
        })
    }

    /// Run the tool calls of one assistant message. Returns the tool_result
    /// blocks (in call order) and whether the turn should stop (rejection).
    fn run_tools(
        &mut self,
        calls: &[(String, String, Value)],
        specs: &[ToolSpec],
        emit: Emit,
    ) -> (Vec<Block>, bool) {
        let mut results: Vec<Option<Block>> = vec![None; calls.len()];
        let mut batch: Vec<usize> = Vec::new();
        let mut stop = false;
        for (i, (id, name, input)) in calls.iter().enumerate() {
            if stop || self.ctx.cancelled() {
                results[i] = Some(Block::ToolResult {
                    tool_use_id: id.clone(),
                    content: if stop {
                        REJECT_MESSAGE.into()
                    } else {
                        INTERRUPT_TOOL_MESSAGE.into()
                    },
                    is_error: true,
                });
                continue;
            }
            emit(Event::ToolStart {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
            });
            // Validate against the schema.
            let spec = specs.iter().find(|s| &s.name == name);
            let pre_error = match spec {
                None => Some(format!("Error: No such tool available: {name}")),
                Some(s) => tools::validate(s, input).err().map(|e| e.to_string()),
            };
            if let Some(err) = pre_error {
                let out = ToolOutput::err(err);
                emit(Event::ToolDone {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                    output: out.clone(),
                });
                results[i] = Some(Block::ToolResult {
                    tool_use_id: id.clone(),
                    content: out.content,
                    is_error: true,
                });
                continue;
            }
            // PreToolUse hooks.
            if !self.hooks.is_empty() {
                let o = self.hooks.run(
                    &self.root,
                    "PreToolUse",
                    Some(name),
                    json!({"session_id": self.session_id, "tool_name": name, "tool_input": input}),
                );
                for n in o.notes {
                    emit(Event::Notice(n));
                }
                if let Some(reason) = o.block {
                    let out =
                        ToolOutput::err(format!("PreToolUse hook blocked this call: {reason}"));
                    emit(Event::ToolDone {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                        output: out.clone(),
                    });
                    results[i] = Some(Block::ToolResult {
                        tool_use_id: id.clone(),
                        content: out.content,
                        is_error: true,
                    });
                    continue;
                }
            }
            let concurrent = matches!(
                name.as_str(),
                "Read" | "Glob" | "Grep" | "LS" | "WebSearch" | "WebFetch" | "Task" | "BashOutput"
            );
            let decision = if name == "ExitPlanMode" {
                Decision::Ask
            } else {
                Checker {
                    rules: &self.rules,
                    mode: self.mode,
                }
                .check(name, input, &self.ctx)
            };
            match decision {
                Decision::Allow if concurrent => {
                    batch.push(i);
                    continue;
                }
                _ => {}
            }
            // Anything else runs in order: flush the parallel batch first.
            self.flush_batch(&mut batch, calls, &mut results, emit);
            let granted = match decision {
                Decision::Allow => Ok(()),
                Decision::Deny(msg) => Err((msg, false)),
                Decision::Ask => self.ask(name, input, emit),
            };
            match granted {
                Ok(()) => {
                    let out = if name == "ExitPlanMode" {
                        ToolOutput::ok("User has approved your plan. You can now start coding. Start with updating your todo list if applicable", Display::Lines { summary: "User approved the plan".into(), body: vec![] })
                    } else {
                        self.exec_tool(id, name, input, emit)
                    };
                    self.after_tool(id, name, input, &out, emit);
                    results[i] = Some(Block::ToolResult {
                        tool_use_id: id.clone(),
                        content: out.content,
                        is_error: out.is_error,
                    });
                }
                Err((msg, halt)) => {
                    let out = ToolOutput {
                        content: msg.clone(),
                        is_error: true,
                        display: Display::Lines {
                            summary: if halt {
                                "User rejected this action".into()
                            } else {
                                msg.lines().next().unwrap_or("Denied").to_string()
                            },
                            body: vec![],
                        },
                    };
                    emit(Event::ToolDone {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                        output: out.clone(),
                    });
                    results[i] = Some(Block::ToolResult {
                        tool_use_id: id.clone(),
                        content: msg,
                        is_error: true,
                    });
                    if halt {
                        stop = true;
                    }
                }
            }
        }
        self.flush_batch(&mut batch, calls, &mut results, emit);
        let blocks = results
            .into_iter()
            .enumerate()
            .map(|(i, r)| {
                r.unwrap_or_else(|| Block::ToolResult {
                    tool_use_id: calls[i].0.clone(),
                    content: INTERRUPT_TOOL_MESSAGE.into(),
                    is_error: true,
                })
            })
            .collect();
        (blocks, stop)
    }

    fn flush_batch(
        &mut self,
        batch: &mut Vec<usize>,
        calls: &[(String, String, Value)],
        results: &mut [Option<Block>],
        emit: Emit,
    ) {
        if batch.is_empty() {
            return;
        }
        let idx = std::mem::take(batch);
        let this: &Engine = self;
        let outs: Vec<(usize, ToolOutput)> = if idx.len() == 1 {
            let i = idx[0];
            vec![(
                i,
                this.exec_tool(&calls[i].0, &calls[i].1, &calls[i].2, emit),
            )]
        } else {
            std::thread::scope(|sc| {
                let hs: Vec<_> = idx
                    .iter()
                    .map(|&i| {
                        let (id, name, input) = &calls[i];
                        sc.spawn(move || (i, this.exec_tool(id, name, input, emit)))
                    })
                    .collect();
                hs.into_iter().filter_map(|h| h.join().ok()).collect()
            })
        };
        for (i, out) in outs {
            let (id, name, input) = &calls[i];
            self.after_tool(id, name, input, &out, emit);
            results[i] = Some(Block::ToolResult {
                tool_use_id: id.clone(),
                content: out.content,
                is_error: out.is_error,
            });
        }
    }

    fn after_tool(&mut self, id: &str, name: &str, input: &Value, out: &ToolOutput, emit: Emit) {
        if let Display::Diff { lines, .. } = &out.display {
            if !out.is_error {
                self.lines_added += lines.iter().filter(|l| l.kind == '+').count();
                self.lines_removed += lines.iter().filter(|l| l.kind == '-').count();
            }
        }
        if name == "TodoWrite" {
            emit(Event::Todos(self.ctx.todos.lock().unwrap().clone()));
        }
        if name == "ExitPlanMode" && !out.is_error && self.mode == Mode::Plan {
            self.mode = Mode::Default;
            emit(Event::ModeChanged(self.mode));
        }
        if !self.hooks.is_empty() {
            let o = self.hooks.run(&self.root, "PostToolUse", Some(name), json!({"session_id": self.session_id, "tool_name": name, "tool_input": input, "tool_response": out.content}));
            for n in o.notes {
                emit(Event::Notice(n));
            }
        }
        emit(Event::ToolDone {
            id: id.to_string(),
            name: name.to_string(),
            input: input.clone(),
            output: out.clone(),
        });
    }

    /// Ask the user. Ok = run it; Err((message for the model, stop turn)).
    fn ask(&mut self, name: &str, input: &Value, emit: Emit) -> Result<(), (String, bool)> {
        if !self.interactive {
            return Err((
                format!("Permission to use {name} was not granted (running non-interactively). Allow it with --allowedTools \"{name}\" or --permission-mode acceptEdits, or add a rule to .ice/settings.json."),
                false,
            ));
        }
        let (detail, always) = match name {
            "Bash" => {
                let cmd = input["command"].as_str().unwrap_or("").to_string();
                let rule = permissions::suggest_bash_rule(&cmd);
                (
                    PermDetail::Command {
                        command: cmd,
                        description: input["description"].as_str().unwrap_or("").to_string(),
                    },
                    Some(format!(
                        "Yes, and don't ask again for {} commands in {}",
                        rule.trim_start_matches("Bash(").trim_end_matches(')'),
                        self.root.display()
                    )),
                )
            }
            "Edit" | "MultiEdit" | "Write" => {
                let (title, lines) = tools::preview(&self.ctx, name, input).unwrap_or_else(|| {
                    (
                        format!("{name} {}", input["file_path"].as_str().unwrap_or("")),
                        vec![],
                    )
                });
                let inside = input["file_path"]
                    .as_str()
                    .map(|p| self.ctx.inside_root(&self.ctx.resolve(p)))
                    .unwrap_or(false);
                (
                    PermDetail::Diff { title, lines },
                    if inside {
                        Some("Yes, allow all edits during this session (shift+tab)".into())
                    } else {
                        None
                    },
                )
            }
            "ExitPlanMode" => (
                PermDetail::Plan(input["plan"].as_str().unwrap_or("").to_string()),
                Some("Yes, and auto-accept edits".into()),
            ),
            "WebFetch" => {
                let url = input["url"].as_str().unwrap_or("").to_string();
                let host = tools::host_of(&url);
                (
                    PermDetail::Fetch { url },
                    Some(format!("Yes, and don't ask again for {host}")),
                )
            }
            n if n.starts_with("mcp__") => (
                PermDetail::Generic {
                    summary: format!("{n}({})", crate::render::compact_json(input, 200)),
                },
                Some(format!(
                    "Yes, and don't ask again for {n} commands in {}",
                    self.root.display()
                )),
            ),
            _ => {
                let target = permissions::target_path(name, input).unwrap_or_default();
                (
                    PermDetail::Generic {
                        summary: format!(
                            "{name}({})",
                            if target.is_empty() {
                                crate::render::compact_json(input, 200)
                            } else {
                                target
                            }
                        ),
                    },
                    Some("Yes, during this session".into()),
                )
            }
        };
        let (tx, rx) = mpsc::channel();
        emit(Event::Permission(PermRequest {
            tool: name.into(),
            detail,
            always_label: always,
            reply: tx,
        }));
        let reply = loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(r) => break r,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if self.ctx.cancelled() {
                        return Err((INTERRUPT_TOOL_MESSAGE.into(), true));
                    }
                }
                Err(_) => return Err((REJECT_MESSAGE.into(), true)),
            }
        };
        match reply {
            PermReply::Allow => Ok(()),
            PermReply::AllowAlways => {
                self.grant_always(name, input, emit);
                Ok(())
            }
            PermReply::Deny(None) => {
                if name == "ExitPlanMode" {
                    Err(("The user doesn't want to proceed with this plan yet. Stay in plan mode and wait for the user's feedback.".into(), true))
                } else {
                    Err((REJECT_MESSAGE.into(), true))
                }
            }
            PermReply::Deny(Some(fb)) => Err((
                format!("The user doesn't want to take this action right now. STOP what you are doing and follow the user's instructions instead.\nThe user said: {fb}"),
                false,
            )),
        }
    }

    fn grant_always(&mut self, name: &str, input: &Value, emit: Emit) {
        match name {
            "Edit" | "MultiEdit" | "Write" => {
                self.mode = Mode::AcceptEdits;
                emit(Event::ModeChanged(self.mode));
            }
            "ExitPlanMode" => {
                self.mode = Mode::AcceptEdits;
                emit(Event::ModeChanged(self.mode));
            }
            "Bash" => {
                let rule = permissions::suggest_bash_rule(input["command"].as_str().unwrap_or(""));
                self.rules.allow.push(rule.clone());
                let _ = crate::settings::add_local_allow(&self.root, &rule);
            }
            "WebFetch" => {
                let rule = format!(
                    "WebFetch(domain:{})",
                    tools::host_of(input["url"].as_str().unwrap_or(""))
                );
                self.rules.allow.push(rule.clone());
                let _ = crate::settings::add_local_allow(&self.root, &rule);
            }
            n if n.starts_with("mcp__") => {
                self.rules.allow.push(n.to_string());
                let _ = crate::settings::add_local_allow(&self.root, n);
            }
            other => {
                // Session-only grant (e.g. reading outside the project).
                let spec = permissions::target_path(other, input)
                    .map(|p| {
                        let abs = self.ctx.resolve(&p);
                        let dir = if abs.is_dir() {
                            abs
                        } else {
                            abs.parent().map(|x| x.to_path_buf()).unwrap_or(abs)
                        };
                        format!(
                            "{other}(/{}/**)",
                            dir.display().to_string().trim_start_matches('/')
                        )
                    })
                    .unwrap_or_else(|| other.to_string());
                self.rules.allow.push(spec.replacen("(/", "(//", 1));
            }
        }
    }

    fn exec_tool(&self, id: &str, name: &str, input: &Value, emit: Emit) -> ToolOutput {
        if name.starts_with("mcp__") {
            return match self.mcp.call(name, input) {
                Ok((text, is_error)) => ToolOutput {
                    display: Display::Lines {
                        summary: String::new(),
                        body: text.lines().map(String::from).collect(),
                    },
                    content: if text.is_empty() {
                        "(no content)".into()
                    } else {
                        tools::clip_middle(&text, 25_000)
                    },
                    is_error,
                },
                Err(e) => ToolOutput::err(e.to_string()),
            };
        }
        match name {
            "Task" => self.run_task(id, input, emit),
            "WebFetch" => self.web_fetch(input),
            _ => tools::run(&self.ctx, name, input),
        }
    }

    fn web_fetch(&self, input: &Value) -> ToolOutput {
        let url = input["url"].as_str().unwrap_or("");
        let prompt_text = input["prompt"].as_str().unwrap_or("").trim();
        let t0 = Instant::now();
        let page = match tools::web_fetch_raw(&self.ctx, url) {
            Ok(p) => p,
            Err(e) => return ToolOutput::err(format!("Failed to fetch {url}: {e}")),
        };
        let size = page.len();
        let summary = format!(
            "Received {} ({:.1}s)",
            human_bytes(size),
            t0.elapsed().as_secs_f32()
        );
        let content = match (&self.cfg, prompt_text.is_empty()) {
            (Some(cfg), false) => {
                let q = format!(
                    "Web page content:\n---\n{}\n---\n\n{prompt_text}\n\nProvide a concise response based only on the content above. Quote exact text only when it is short, and never reproduce song lyrics.",
                    page.chars().take(60_000).collect::<String>()
                );
                match model::complete_text(
                    cfg,
                    "You answer questions about a fetched web page.",
                    &q,
                    &self.ctx.cancel,
                ) {
                    Ok((t, _)) => t,
                    Err(_) => page.chars().take(20_000).collect(),
                }
            }
            _ => page.chars().take(20_000).collect(),
        };
        ToolOutput::ok(
            content,
            Display::Lines {
                summary,
                body: vec![],
            },
        )
    }

    /// Task tool: run a sub-agent to completion in an isolated context.
    fn run_task(&self, id: &str, input: &Value, emit: Emit) -> ToolOutput {
        let Some(cfg) = self.cfg.clone() else {
            return ToolOutput::err("No model configured");
        };
        let kind = input["subagent_type"].as_str().unwrap_or("general-purpose");
        let Some(def) = agents::find(&self.root, kind) else {
            let names: Vec<String> = agents::all(&self.root)
                .into_iter()
                .map(|a| a.name)
                .collect();
            return ToolOutput::err(format!(
                "Agent type '{kind}' not found. Available agents: {}",
                names.join(", ")
            ));
        };
        let prompt_text = input["prompt"].as_str().unwrap_or("").to_string();
        let cfg = match &def.model {
            Some(m) => ModelConfig::resolve(Some(m)).unwrap_or(cfg),
            None => cfg,
        };
        let mut child = Engine {
            root: self.root.clone(),
            cfg: Some(cfg.clone()),
            messages: Vec::new(),
            ctx: self.ctx.clone(),
            mcp: self.mcp.clone(),
            hooks: self.hooks.clone(),
            mode: if self.mode == Mode::Plan {
                Mode::Plan
            } else {
                self.mode
            },
            rules: self.rules.clone(),
            interactive: self.interactive,
            max_turns: Some(100),
            usage: Usage::default(),
            cost: 0.0,
            cost_known: true,
            last_context: 0,
            transcript: Transcript::sidechain(&self.root, &self.session_id, id),
            session_id: self.session_id.clone(),
            text_tools: self.text_tools,
            auto_compact: false,
            goal: None,
            append_system: None,
            replace_system: None,
            git_status: None,
            subagent: Some(def.clone()),
            api_ms: 0,
            started: Instant::now(),
            lines_added: 0,
            lines_removed: 0,
            pending_usage: std::sync::Mutex::new(Usage::default()),
            prompt_marks: Vec::new(),
        };
        let parent = id.to_string();
        let tool_count = std::sync::atomic::AtomicUsize::new(0);
        let child_usage = std::sync::Mutex::new(Usage::default());
        let t0 = Instant::now();
        let sub_emit = |ev: Event| match ev {
            Event::ToolStart { name, input, .. } => {
                tool_count.fetch_add(1, Ordering::Relaxed);
                emit(Event::SubagentStep {
                    parent: parent.clone(),
                    line: crate::render::tool_call_title(&name, &input, &self.root),
                });
            }
            Event::Permission(p) => emit(Event::Permission(p)),
            Event::Retry {
                attempt,
                error,
                wait,
            } => emit(Event::Retry {
                attempt,
                error,
                wait,
            }),
            Event::Usage { turn, .. } => child_usage.lock().unwrap().add(&turn),
            _ => {}
        };
        child.record(Message::user_text(prompt_text));
        let r = child.run_loop(&cfg, 0, &sub_emit);
        let u = *child_usage.lock().unwrap();
        self.pending_usage.lock().unwrap().add(&u);
        let n = tool_count.load(Ordering::Relaxed);
        let secs = t0.elapsed().as_secs_f32();
        let tokens = u.input_tokens
            + u.output_tokens
            + u.cache_read_input_tokens
            + u.cache_creation_input_tokens;
        let summary = format!(
            "Done ({n} tool use{} · {} tokens · {secs:.1}s)",
            if n == 1 { "" } else { "s" },
            crate::render::human_tokens(tokens)
        );
        match r {
            Ok(res) => ToolOutput::ok(
                if res.text.trim().is_empty() {
                    "(the agent finished without a final message)".into()
                } else {
                    res.text
                },
                Display::Lines {
                    summary,
                    body: vec![],
                },
            ),
            Err(e) if http::is_interrupted(&e) => ToolOutput::err(INTERRUPT_TOOL_MESSAGE),
            Err(e) => ToolOutput::err(format!("Agent failed: {e}")),
        }
    }

    /// Summarise the conversation and replace it with the summary.
    pub fn compact(&mut self, instructions: Option<&str>, emit: Emit) -> Result<()> {
        let Some(cfg) = self.cfg.clone() else {
            return Err(anyhow!("No model configured"));
        };
        if self.messages.is_empty() {
            return Err(anyhow!("Nothing to compact yet"));
        }
        let mut msgs = self.messages.clone();
        store::repair(&mut msgs);
        let mut ask = prompt::COMPACT_PROMPT.to_string();
        if let Some(i) = instructions.filter(|s| !s.trim().is_empty()) {
            ask.push_str(&format!("\n\nAdditional instructions from the user: {i}"));
        }
        msgs.push(Message::user_text(ask));
        store::repair(&mut msgs);
        let system = "You are a helpful AI assistant tasked with summarizing conversations.";
        let r = model::stream_turn(
            &cfg,
            &TurnRequest {
                system,
                messages: &msgs,
                tools: &[],
                thinking_budget: 0,
                text_tools: false,
            },
            &self.ctx.cancel,
            &mut |_| {},
        )?;
        self.emit_usage(r.usage, emit);
        let summary: String = r
            .content
            .iter()
            .filter_map(|b| {
                if let Block::Text { text } = b {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect();
        if summary.trim().is_empty() {
            return Err(anyhow!("The model returned an empty summary"));
        }
        self.transcript.compact(&summary);
        self.messages = vec![store::compact_message(&summary)];
        self.prompt_marks.clear();
        // Re-attach the todo list so progress tracking survives.
        let todos = self.ctx.todos.lock().unwrap().clone();
        if !todos.is_empty() {
            if let Some(m) = self.messages.first_mut() {
                m.content.push(Block::Text {
                    text: format!(
                        "<system-reminder>Current todo list: {}</system-reminder>",
                        serde_json::to_string(&todos).unwrap_or_default()
                    ),
                });
            }
        }
        self.last_context = self.messages.iter().map(|m| m.approx_tokens()).sum();
        Ok(())
    }

    fn run_cli_agent(
        &mut self,
        agent: &crate::providers::CliAgent,
        prompt_text: &str,
        emit: Emit,
    ) -> Result<TurnResult> {
        if !crate::providers::cli_agent_present(agent) {
            let msg = format!(
                "`{}` isn't on PATH. Install it, then: {}",
                agent.bin, agent.login_hint
            );
            emit(Event::Error(msg.clone()));
            return Ok(TurnResult {
                text: msg,
                is_error: true,
                ..Default::default()
            });
        }
        emit(Event::Requesting);
        let id = crate::conv::new_id("cli");
        let input = json!({"command": format!("{} {}", agent.bin, agent.args.join(" ").replace("{PROMPT}", "…"))});
        emit(Event::ToolStart {
            id: id.clone(),
            name: "Bash".into(),
            input: input.clone(),
        });
        let r = crate::cli::complete(agent, prompt_text, &self.root, &self.ctx.cancel);
        match r {
            Ok(text) => {
                emit(Event::ToolDone {
                    id,
                    name: "Bash".into(),
                    input,
                    output: ToolOutput::ok(
                        "",
                        Display::Lines {
                            summary: format!("{} replied", agent.name),
                            body: vec![],
                        },
                    ),
                });
                self.record(Message::user_text(prompt_text));
                self.record(Message::assistant_text(text.clone()));
                emit(Event::Text(text.clone()));
                Ok(TurnResult {
                    text,
                    num_turns: 1,
                    ..Default::default()
                })
            }
            Err(e) => {
                emit(Event::ToolDone {
                    id,
                    name: "Bash".into(),
                    input,
                    output: ToolOutput::err(e.to_string()),
                });
                Err(e)
            }
        }
    }
}

/// A user text block typed by the person (not a reminder, marker or bash echo).
pub fn is_user_prompt(text: &str) -> bool {
    let t = text.trim_start();
    !(t.is_empty()
        || t.starts_with("<system-reminder>")
        || t.starts_with("<bash-")
        || t.starts_with("[Request interrupted")
        || t.starts_with("This session is being continued")
        || t.starts_with("Stop hook feedback"))
}

fn split_tool_list(s: &str) -> Vec<String> {
    // "Bash(git log:*) Edit,Read" → ["Bash(git log:*)", "Edit", "Read"]
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0;
    for c in s.chars() {
        match c {
            '(' => {
                depth += 1;
                cur.push(c)
            }
            ')' => {
                depth -= 1;
                cur.push(c)
            }
            ',' | ' ' if depth == 0 => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

pub fn human_bytes(n: usize) -> String {
    if n >= 1 << 20 {
        format!("{:.1}MB", n as f64 / (1 << 20) as f64)
    } else if n >= 1024 {
        format!("{:.1}KB", n as f64 / 1024.0)
    } else {
        format!("{n} bytes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_lists_split_like_the_cli() {
        assert_eq!(
            split_tool_list("Bash(git log:*) Edit,Read"),
            vec!["Bash(git log:*)", "Edit", "Read"]
        );
        assert_eq!(
            split_tool_list("Bash(npm run test:*)"),
            vec!["Bash(npm run test:*)"]
        );
    }
}
