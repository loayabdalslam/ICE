//! The interactive REPL: an inline, Claude-Code-style terminal UI.
//!
//! Finished items (your prompts, answers, tool calls with results) go into
//! normal scrollback. The bottom "live" region holds whatever is still in
//! flight: streaming text, running tools, the spinner, todos, the input box,
//! permission dialogs and pickers. The engine runs on a worker thread.

use crate::conv::{Block, Message, Role};
use crate::editor::Editor;
use crate::engine::{self, Engine, Event, PermReply, PermRequest, Stats};
use crate::permissions::Mode;
use crate::render::{self, str_w, Line, Style};
use crate::term::Screen;
use crate::theme::{Theme, ThemeName};
use crate::tools::{Display, TodoItem, ToolCtx, ToolOutput};
use crossterm::event::{
    self as cev, Event as CEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub enum Cmd {
    Submit(String),
    Bash(String),
    Compact(Option<String>),
    Clear,
    Model(String),
    Mode(Mode),
    Resume(String),
    Rewind(usize),
    ReloadModel,
    ReloadRules,
    Goal(Option<String>),
    Context,
}

#[derive(Clone, Debug)]
pub enum Item {
    Welcome,
    User(String),
    Bash {
        command: String,
        output: ToolOutput,
    },
    Assistant(String),
    Thinking(String),
    Tool {
        id: String,
        name: String,
        input: Value,
        output: Option<ToolOutput>,
        steps: Vec<String>,
    },
    Info(String),
    Notice(String),
    Error(String),
    Interrupted,
}

pub enum PickerKind {
    Model(Vec<String>),
    Resume(Vec<crate::store::SessionInfo>),
    Theme,
    Login,
    LoginKey { provider: usize },
    LoginUrl { provider: usize },
    LoginMethod { provider: usize },
    OAuthWait,
    Memory(String),
    OpenMemory,
    Rewind(Vec<(usize, String)>),
    Config,
    Trust,
    Bashes(Vec<String>),
}

pub struct Picker {
    pub title: String,
    pub body: Vec<Line>,
    pub options: Vec<(String, String)>,
    pub idx: usize,
    pub kind: PickerKind,
    /// Text entry pickers (API key, URL, model name).
    pub entry: Option<Editor>,
    pub masked: bool,
}

pub struct PermState {
    pub req: PermRequest,
    pub idx: usize,
}

pub enum Bg {
    Models(Result<Vec<String>, String>),
    Update(String),
    OAuth(usize, Result<crate::oauth::Tokens, String>),
}

pub struct Repl {
    pub(crate) scr: Screen,
    pub(crate) theme: Theme,
    pub(crate) theme_name: ThemeName,
    pub(crate) root: PathBuf,
    pub(crate) tx: Sender<Cmd>,
    pub(crate) rx: Receiver<Event>,
    pub(crate) bg_tx: Sender<Bg>,
    pub(crate) bg_rx: Receiver<Bg>,
    pub(crate) cancel: Arc<AtomicBool>,
    pub(crate) ctx: Option<Arc<ToolCtx>>,
    pub(crate) mcp: Option<Arc<crate::mcp::Pool>>,
    pub(crate) stats: Stats,
    pub(crate) ready: bool,

    pub(crate) items: Vec<Item>,
    pub(crate) committed: usize,
    pub(crate) streaming: String,
    pub(crate) thinking_stream: String,

    pub(crate) running: bool,
    pub(crate) run_started: Instant,
    pub(crate) verb: &'static str,
    pub(crate) stream_chars: usize,
    pub(crate) retry: Option<String>,
    pub(crate) interrupting: bool,
    pub(crate) mode: Mode,
    pub(crate) bypass_available: bool,
    pub(crate) todos: Vec<TodoItem>,
    pub(crate) show_todos: bool,

    pub(crate) input: Editor,
    pub(crate) history: Vec<String>,
    pub(crate) hist_idx: Option<usize>,
    pub(crate) hist_draft: String,
    pub(crate) pastes: Vec<String>,
    pub(crate) queued: Vec<String>,

    pub(crate) perm: Option<PermState>,
    pub(crate) picker: Option<Picker>,
    pub(crate) shortcuts: bool,
    pub(crate) sugg: Vec<(String, String)>,
    pub(crate) sugg_idx: usize,
    pub(crate) sugg_dismissed: bool,
    pub(crate) ctrl_c_at: Option<Instant>,
    pub(crate) esc_at: Option<Instant>,
    pub(crate) verbose: bool,
    pub(crate) show_thinking: bool,
    pub(crate) transcript_mode: bool,
    pub(crate) flash: Option<(String, Instant)>,
    pub(crate) update_note: Option<String>,
    pub(crate) quit: bool,
    pub(crate) dirty: bool,
    pub(crate) tick: u64,
    pub(crate) files_cache: Option<(Instant, Vec<String>)>,
    pub(crate) oauth_cancel: Arc<AtomicBool>,
    pub(crate) goal: Option<String>,
}

const VERBS: &[&str] = &[
    "Freezing",
    "Crystallizing",
    "Chilling",
    "Frosting",
    "Glaciating",
    "Condensing",
    "Solidifying",
    "Snowballing",
    "Drifting",
    "Icing",
    "Shimmering",
    "Sculpting",
    "Polishing",
    "Pondering",
    "Computing",
    "Brewing",
    "Tinkering",
    "Forging",
    "Calibrating",
    "Distilling",
    "Mulling",
    "Cooling",
    "Tempering",
    "Refracting",
];
const SPINNER: &[&str] = &["·", "✢", "✳", "❄", "✻", "✽", "✻", "❄", "✳", "✢"];

pub struct StartOptions {
    pub root: PathBuf,
    pub engine: engine::Options,
    pub initial_prompt: Option<String>,
    pub resume: Option<String>,
    pub continue_last: bool,
    pub pick_resume: bool,
    pub bypass_available: bool,
    pub verbose: bool,
}

fn spawn_worker(
    mut opts: engine::Options,
    resume: Option<String>,
    events: Sender<Event>,
) -> Sender<Cmd> {
    let (tx, rx) = mpsc::channel::<Cmd>();
    opts.interactive = true;
    std::thread::spawn(move || {
        let emit_tx = events.clone();
        let emit = move |e: Event| {
            let _ = emit_tx.send(e);
        };
        let mut eng = Engine::new(opts);
        if let Some(id) = resume {
            match eng.resume(&id) {
                Ok(id) => emit(Event::Resumed {
                    id,
                    messages: eng.messages.clone(),
                }),
                Err(e) => emit(Event::Error(e.to_string())),
            }
        }
        if !eng.hooks.is_empty() {
            let o = eng.hooks.run(
                &eng.root,
                "SessionStart",
                None,
                serde_json::json!({"session_id": eng.session_id, "source": "startup"}),
            );
            for n in o.notes {
                emit(Event::Notice(n));
            }
        }
        emit(Event::Ready(Box::new(engine::Ready {
            ctx: eng.ctx.clone(),
            mcp: eng.mcp.clone(),
            stats: eng.stats(),
        })));
        for cmd in rx {
            match cmd {
                Cmd::Submit(p) => {
                    eng.submit(&p, &emit);
                }
                Cmd::Bash(c) => {
                    let out = eng.user_bash(&c);
                    emit(Event::BashDone {
                        command: c,
                        output: out,
                    });
                }
                Cmd::Compact(i) => {
                    eng.ctx.cancel.store(false, Ordering::Relaxed);
                    match eng.compact(i.as_deref(), &emit) {
                        Ok(()) => emit(Event::Compacted { auto: false }),
                        Err(e) => emit(Event::Error(format!("Error during compaction: {e}"))),
                    }
                    emit(Event::Done(Default::default()));
                }
                Cmd::Clear => {
                    eng.clear();
                    emit(Event::Cleared);
                }
                Cmd::Model(m) => {
                    eng.set_model(&m);
                }
                Cmd::Mode(m) => eng.mode = m,
                Cmd::Resume(id) => match eng.resume(&id) {
                    Ok(id) => emit(Event::Resumed {
                        id,
                        messages: eng.messages.clone(),
                    }),
                    Err(e) => emit(Event::Error(e.to_string())),
                },
                Cmd::Rewind(k) => {
                    if eng.rewind(k) {
                        emit(Event::Rewound);
                    }
                }
                Cmd::ReloadModel => eng.reload_model(),
                Cmd::Goal(g) => eng.goal = g,
                Cmd::ReloadRules => {
                    let s = crate::settings::Settings::load(&eng.root);
                    eng.rules = crate::permissions::Rules {
                        allow: s.allow,
                        deny: s.deny,
                        ask: s.ask,
                    };
                }
                Cmd::Context => {
                    let parts = eng.context_breakdown();
                    emit(Event::Info(crate::commands::context_report(
                        &parts,
                        eng.last_context,
                        eng.window(),
                        &eng.model_name(),
                    )));
                }
            }
            emit(Event::Stats(eng.stats()));
        }
    });
    tx
}

impl Repl {
    pub fn run(o: StartOptions) -> anyhow::Result<()> {
        let settings = crate::settings::Settings::load(&o.root);
        let theme_name = settings
            .theme
            .as_deref()
            .and_then(ThemeName::parse)
            .unwrap_or(ThemeName::DEFAULT);
        let cancel = o.engine.cancel.clone();
        let mode = o.engine.mode;
        let (ev_tx, ev_rx) = mpsc::channel();
        let (bg_tx, bg_rx) = mpsc::channel();

        // Resolve which session to resume before the worker starts.
        let mut resume = o.resume.clone();
        if o.continue_last && resume.is_none() {
            resume = crate::store::list(&o.root).first().map(|s| s.id.clone());
        }
        let tx = spawn_worker(o.engine, resume.clone(), ev_tx);

        let user_verbose = crate::settings::get_user_value("verbose")
            .as_bool()
            .unwrap_or(false);
        let show_thinking = crate::settings::get_user_value("showThinking")
            .as_bool()
            .unwrap_or(true);
        let mut r = Repl {
            scr: Screen::new(),
            theme: theme_name.palette(),
            theme_name,
            root: o.root.clone(),
            tx,
            rx: ev_rx,
            bg_tx,
            bg_rx,
            cancel,
            ctx: None,
            mcp: None,
            stats: Stats {
                mode: Some(mode),
                ..Default::default()
            },
            ready: false,
            items: vec![Item::Welcome],
            committed: 0,
            streaming: String::new(),
            thinking_stream: String::new(),
            running: false,
            run_started: Instant::now(),
            verb: VERBS[0],
            stream_chars: 0,
            retry: None,
            interrupting: false,
            mode,
            bypass_available: o.bypass_available,
            todos: Vec::new(),
            show_todos: true,
            input: Editor::default(),
            history: load_history(),
            hist_idx: None,
            hist_draft: String::new(),
            pastes: Vec::new(),
            queued: Vec::new(),
            perm: None,
            picker: None,
            shortcuts: false,
            sugg: Vec::new(),
            sugg_idx: 0,
            sugg_dismissed: false,
            ctrl_c_at: None,
            esc_at: None,
            verbose: o.verbose || user_verbose,
            show_thinking,
            transcript_mode: false,
            flash: None,
            update_note: None,
            quit: false,
            dirty: true,
            tick: 0,
            files_cache: None,
            oauth_cancel: Arc::new(AtomicBool::new(false)),
            goal: None,
        };

        crossterm::terminal::enable_raw_mode()?;
        let _ = crossterm::execute!(std::io::stdout(), cev::EnableBracketedPaste);
        let kb = crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
        if kb {
            let _ = crossterm::execute!(
                std::io::stdout(),
                cev::PushKeyboardEnhancementFlags(
                    cev::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                )
            );
        }
        install_panic_hook(kb);

        if !crate::providers::IceConfig::is_trusted(&r.root)
            && std::env::var_os("ICE_SKIP_TRUST").is_none()
        {
            r.open_trust();
        } else if crate::model::ModelConfig::resolve(None).is_none()
            && crate::providers::selected_cli_agent().is_none()
        {
            r.open_login();
        }
        if o.pick_resume {
            r.open_resume();
        }
        if settings.auto_update && std::env::var_os("ICE_NO_AUTO_UPDATE").is_none() {
            let btx = r.bg_tx.clone();
            std::thread::spawn(move || {
                if let Some(note) = crate::update::background_check() {
                    let _ = btx.send(Bg::Update(note));
                }
            });
        }
        if let Some(p) = o.initial_prompt {
            r.submit_text(p);
        }
        let res = r.event_loop();
        let _ = r.scr.finish();
        if kb {
            let _ = crossterm::execute!(std::io::stdout(), cev::PopKeyboardEnhancementFlags);
        }
        let _ = crossterm::execute!(std::io::stdout(), cev::DisableBracketedPaste);
        crossterm::terminal::disable_raw_mode()?;
        if let Some(ctx) = &r.ctx {
            ctx.kill_background();
        }
        if !r.stats.session_id.is_empty() && r.stats.messages > 0 {
            println!(
                "Resume this session with:\n  ice --resume {}",
                r.stats.session_id
            );
        }
        res
    }

    fn event_loop(&mut self) -> anyhow::Result<()> {
        let mut last_frame = Instant::now();
        loop {
            self.pump_engine();
            self.pump_bg();
            self.commit_ready()?;
            if self.quit {
                self.render_live()?;
                return Ok(());
            }
            let animating = self.running || self.flash.is_some() || self.ctrl_c_at.is_some();
            if self.dirty || (animating && last_frame.elapsed() >= Duration::from_millis(100)) {
                self.tick = self.tick.wrapping_add(1);
                self.render_live()?;
                self.dirty = false;
                last_frame = Instant::now();
            }
            if cev::poll(Duration::from_millis(33))? {
                // Drain everything queued (fast typing / paste bursts).
                loop {
                    let ev = cev::read()?;
                    self.on_terminal_event(ev);
                    if !cev::poll(Duration::from_millis(0))? {
                        break;
                    }
                }
                self.dirty = true;
            }
            if let Some((_, at)) = &self.flash {
                if at.elapsed() > Duration::from_secs(3) {
                    self.flash = None;
                    self.dirty = true;
                }
            }
            if let Some(at) = self.ctrl_c_at {
                if at.elapsed() > Duration::from_millis(1500) {
                    self.ctrl_c_at = None;
                    self.dirty = true;
                }
            }
        }
    }

    // ─────────────────────────── engine events ───────────────────────────

    fn pump_engine(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            self.dirty = true;
            self.on_engine_event(ev);
        }
    }

    fn pump_bg(&mut self) {
        while let Ok(b) = self.bg_rx.try_recv() {
            self.dirty = true;
            match b {
                Bg::Models(Ok(list)) => {
                    if let Some(p) = &mut self.picker {
                        if let PickerKind::Model(existing) = &mut p.kind {
                            for m in list {
                                if !existing.contains(&m) {
                                    existing.push(m.clone());
                                    p.options.push((m, String::new()));
                                }
                            }
                        }
                    }
                }
                Bg::Models(Err(e)) => self.flash(format!("Model discovery failed: {e}")),
                Bg::Update(note) => self.update_note = Some(note),
                Bg::OAuth(provider, res) => {
                    if matches!(
                        self.picker.as_ref().map(|p| &p.kind),
                        Some(PickerKind::OAuthWait)
                    ) {
                        self.picker = None;
                    }
                    crate::commands::oauth_finished(self, provider, res);
                }
            }
        }
    }

    fn find_tool(&mut self, id: &str) -> Option<&mut Item> {
        self.items
            .iter_mut()
            .rev()
            .find(|i| matches!(i, Item::Tool { id: tid, .. } if tid == id))
    }

    fn on_engine_event(&mut self, ev: Event) {
        match ev {
            Event::Requesting => {
                self.retry = None;
            }
            Event::TextDelta(t) => {
                self.stream_chars += t.len();
                self.streaming.push_str(&t);
            }
            Event::ThinkingDelta(t) => {
                self.stream_chars += t.len();
                self.thinking_stream.push_str(&t);
            }
            Event::Thinking(t) => {
                self.thinking_stream.clear();
                if self.show_thinking {
                    self.items.push(Item::Thinking(t));
                }
            }
            Event::Text(t) => {
                self.streaming.clear();
                self.items.push(Item::Assistant(t));
            }
            Event::ToolStart { id, name, input } => {
                self.streaming.clear();
                if self.find_tool(&id).is_none() {
                    self.items.push(Item::Tool {
                        id,
                        name,
                        input,
                        output: None,
                        steps: vec![],
                    });
                }
            }
            Event::ToolDone {
                id,
                name,
                input,
                output,
            } => {
                if let Some(Item::Tool { output: o, .. }) = self.find_tool(&id) {
                    *o = Some(output);
                } else {
                    self.items.push(Item::Tool {
                        id,
                        name,
                        input,
                        output: Some(output),
                        steps: vec![],
                    });
                }
            }
            Event::SubagentStep { parent, line } => {
                if let Some(Item::Tool { steps, .. }) = self.find_tool(&parent) {
                    steps.push(line);
                }
            }
            Event::Permission(req) => {
                let _ = crossterm::execute!(std::io::stdout(), crossterm::style::Print("\x07"));
                self.perm = Some(PermState { req, idx: 0 });
            }
            Event::Retry {
                attempt,
                error,
                wait,
            } => {
                self.retry = Some(format!(
                    "{} · Retrying in {}s… (attempt {attempt}/8)",
                    render::truncate(&error, 60),
                    wait.as_secs().max(1)
                ));
            }
            Event::Usage {
                context,
                window,
                cost,
                total,
                ..
            } => {
                self.stats.context = context;
                self.stats.window = window;
                self.stats.cost = cost;
                self.stats.usage = total;
            }
            Event::Todos(t) => self.todos = t,
            Event::ModeChanged(m) => self.mode = m,
            Event::Compacted { auto } => {
                self.items.push(Item::Info(if auto {
                    "Conversation compacted automatically (ctrl+r to see full summary)".into()
                } else {
                    "Compacted. ctrl+r to see full summary".into()
                }));
            }
            Event::Notice(n) => self.items.push(Item::Notice(n)),
            Event::Error(e) => self.items.push(Item::Error(e)),
            Event::Done(r) => {
                self.running = false;
                self.interrupting = false;
                self.retry = None;
                // Keep whatever streamed before an interrupt.
                if r.interrupted && !self.streaming.trim().is_empty() {
                    let partial = std::mem::take(&mut self.streaming);
                    self.items.push(Item::Assistant(partial));
                }
                self.streaming.clear();
                self.thinking_stream.clear();
                self.perm = None;
                // Close any tool left running (interrupt).
                for it in self.items.iter_mut() {
                    if let Item::Tool { output, .. } = it {
                        if output.is_none() {
                            *output = Some(ToolOutput::err("Interrupted"));
                        }
                    }
                }
                if r.interrupted {
                    self.items.push(Item::Interrupted);
                }
                if self.todos.iter().all(|t| t.status == "completed") {
                    self.todos.clear();
                }
                let _ = crossterm::execute!(std::io::stdout(), crossterm::style::Print("\x07"));
                if !self.queued.is_empty() {
                    let q = std::mem::take(&mut self.queued).join("\n");
                    self.submit_text(q);
                }
            }
            Event::Ready(r) => {
                self.ctx = Some(r.ctx.clone());
                self.mcp = Some(r.mcp.clone());
                self.stats = r.stats.clone();
                self.ready = true;
                let failed: Vec<String> = r
                    .mcp
                    .status
                    .iter()
                    .filter(|(_, s)| s.is_err())
                    .map(|(n, _)| n.clone())
                    .collect();
                if !failed.is_empty() {
                    self.items.push(Item::Notice(format!(
                        "MCP server{} failed to connect: {} · /mcp for details",
                        if failed.len() == 1 { "" } else { "s" },
                        failed.join(", ")
                    )));
                }
            }
            Event::Stats(s) => {
                self.mode = s.mode.unwrap_or(self.mode);
                self.stats = s;
            }
            Event::Info(t) => self.items.push(Item::Info(t)),
            Event::Resumed { id, messages } => {
                self.items.truncate(1);
                self.items.extend(items_from_messages(&messages));
                self.items.push(Item::Info(format!("Resumed session {id}")));
                self.redraw_all();
            }
            Event::BashDone { command, output } => {
                self.running = false;
                self.items.push(Item::Bash { command, output });
            }
            Event::Cleared => {
                self.items = vec![Item::Welcome];
                self.todos.clear();
                self.redraw_all();
            }
            Event::Rewound => {}
        }
    }

    pub(crate) fn flash(&mut self, s: impl Into<String>) {
        self.flash = Some((s.into(), Instant::now()));
        self.dirty = true;
    }

    pub(crate) fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }

    // ─────────────────────────── rendering ───────────────────────────

    fn item_complete(it: &Item) -> bool {
        !matches!(it, Item::Tool { output: None, .. })
    }

    /// Print every finished item (in order) into scrollback.
    fn commit_ready(&mut self) -> anyhow::Result<()> {
        if self.transcript_mode {
            return Ok(());
        }
        let mut lines = Vec::new();
        while self.committed < self.items.len() && Self::item_complete(&self.items[self.committed])
        {
            lines.extend(self.render_item(&self.items[self.committed], false));
            self.committed += 1;
        }
        if !lines.is_empty() {
            self.scr.commit(&lines)?;
            self.dirty = true;
        }
        Ok(())
    }

    /// Clear the screen and reprint the whole transcript (resize, /clear,
    /// resume, transcript mode).
    pub(crate) fn redraw_all(&mut self) {
        self.scr.refresh_size();
        let _ = self.scr.clear_all();
        self.committed = 0;
        if self.transcript_mode {
            let mut lines = vec![
                Line::styled(
                    "  Showing detailed transcript · ctrl+r to toggle",
                    Style::fg(self.theme.dim),
                ),
                Line::new(),
            ];
            for it in &self.items {
                lines.extend(self.render_item(it, true));
            }
            let _ = self.scr.commit(&lines);
            self.committed = self.items.len();
        }
        self.dirty = true;
    }

    fn render_item(&self, it: &Item, verbose: bool) -> Vec<Line> {
        let w = self.scr.cols();
        let t = &self.theme;
        let verbose = verbose || self.verbose;
        let mut v = Vec::new();
        match it {
            Item::Welcome => v.extend(self.welcome_lines()),
            Item::User(text) => {
                for (i, l) in text.lines().enumerate() {
                    let pre = if i == 0 { "> " } else { "  " };
                    v.extend(render::wrap(
                        &Line::styled(format!("{pre}{l}"), Style::fg(t.user)),
                        w,
                        2,
                    ));
                }
            }
            Item::Bash { command, output } => {
                v.push(
                    Line::styled("! ", Style::fg(t.bash)).push(command.clone(), Style::fg(t.user)),
                );
                v.extend(render::tool_result("Bash", output, w, t, verbose));
            }
            Item::Assistant(text) => {
                let md = render::markdown(text, w, "  ", t);
                for (i, mut l) in md.into_iter().enumerate() {
                    if i == 0 {
                        l.spans[0] = render::Span {
                            text: "⏺ ".into(),
                            style: Style::default(),
                        };
                    }
                    v.push(l);
                }
            }
            Item::Thinking(text) => {
                v.push(Line::styled("✻ Thinking…", Style::fg(t.dim).italic()));
                let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
                let cap = if verbose { lines.len() } else { 12 };
                for l in lines.iter().take(cap) {
                    v.extend(render::wrap(
                        &Line::styled(format!("  {l}"), Style::fg(t.dim).italic()),
                        w,
                        2,
                    ));
                }
                if lines.len() > cap {
                    v.push(Line::styled(
                        format!("  … +{} lines (ctrl+r to expand)", lines.len() - cap),
                        Style::fg(t.dim),
                    ));
                }
            }
            Item::Tool {
                name,
                input,
                output,
                steps,
                ..
            } => {
                let title = render::tool_call_title(name, input, &self.root);
                let dot = match output {
                    None => {
                        if self.tick % 8 < 4 {
                            Style::fg(t.dim)
                        } else {
                            Style::default()
                        }
                    }
                    Some(o) if o.is_error => Style::fg(t.err),
                    Some(_) => Style::fg(t.ok),
                };
                v.extend(render::tool_header(&title, dot, w));
                if name == "Task" {
                    let shown: Vec<&String> = if verbose {
                        steps.iter().collect()
                    } else {
                        steps
                            .iter()
                            .rev()
                            .take(3)
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev()
                            .collect()
                    };
                    for s in shown {
                        v.extend(render::wrap(
                            &Line::styled("  ⎿  ", Style::fg(t.dim))
                                .push(s.clone(), Style::fg(t.dim)),
                            w,
                            5,
                        ));
                    }
                    if !verbose && steps.len() > 3 {
                        v.push(Line::styled(
                            format!(
                                "     +{} more tool use{} (ctrl+r to expand)",
                                steps.len() - 3,
                                if steps.len() == 4 { "" } else { "s" }
                            ),
                            Style::fg(t.dim),
                        ));
                    }
                }
                match output {
                    None => {
                        let what = match name.as_str() {
                            "Task" if !steps.is_empty() => None,
                            "Bash" => Some("Running…"),
                            "Task" => Some("Initializing…"),
                            _ => Some("Running…"),
                        };
                        if self
                            .perm
                            .as_ref()
                            .map(|p| p.req.tool == *name)
                            .unwrap_or(false)
                        {
                            // The dialog below explains what is pending.
                        } else if let Some(what) = what {
                            v.push(
                                Line::styled("  ⎿  ", Style::fg(t.dim))
                                    .push(what, Style::fg(t.dim)),
                            );
                        }
                    }
                    Some(o) => v.extend(render::tool_result(name, o, w, t, verbose)),
                }
            }
            Item::Info(text) => {
                for (i, l) in text.lines().enumerate() {
                    let pre = if i == 0 { "  ⎿  " } else { "     " };
                    v.extend(render::wrap(
                        &Line::styled(pre, Style::fg(t.dim)).push(l.to_string(), Style::default()),
                        w,
                        5,
                    ));
                }
            }
            Item::Notice(n) => v.extend(render::wrap(
                &Line::styled(format!("  ⎿  {n}"), Style::fg(t.dim)),
                w,
                5,
            )),
            Item::Error(e) => {
                for (i, l) in e.lines().enumerate() {
                    let pre = if i == 0 { "  ⎿  " } else { "     " };
                    let text = l.to_string();
                    v.extend(render::wrap(
                        &Line::styled(pre, Style::fg(t.dim)).push(text, Style::fg(t.err)),
                        w,
                        5,
                    ));
                }
            }
            Item::Interrupted => {
                v.push(
                    Line::styled("  ⎿  ", Style::fg(t.dim))
                        .push("Interrupted by user", Style::fg(t.err))
                        .push(" · What should ICE do instead?", Style::fg(t.dim)),
                );
            }
        }
        if !matches!(it, Item::Welcome) {
            v.push(Line::new());
        }
        v
    }

    fn connected(&self) -> bool {
        self.stats.configured
            || crate::model::ModelConfig::resolve(None).is_some()
            || crate::providers::selected_cli_agent().is_some()
    }

    /// The ICE home screen: header, FLOE + wordmark, and three cards.
    fn welcome_lines(&self) -> Vec<Line> {
        let t = &self.theme;
        let w = self.scr.cols();
        let dim = Style::fg(t.dim);
        let live = self.connected();
        let cfg = crate::model::ModelConfig::resolve(None);
        let provider = cfg
            .as_ref()
            .map(|c| c.provider_id.clone())
            .or_else(|| crate::providers::selected_cli_agent().map(|a| format!("cli:{}", a.id)))
            .unwrap_or_else(|| crate::providers::IceConfig::load().provider);
        let model = cfg
            .as_ref()
            .map(|c| c.model.clone())
            .unwrap_or_else(|| "your-configured-model".into());
        let mut out = Vec::new();

        // Header: ■ ICE / version ……… ~/path · LIVE
        let home = crate::tools::home_dir();
        let path = match self.root.strip_prefix(&home) {
            Ok(rel) => format!("~/{}", rel.display()),
            Err(_) => self.root.display().to_string(),
        };
        let mut head = Line::styled("  ■ ", Style::fg(t.accent))
            .push("ICE", Style::fg(t.accent).bold())
            .push(format!("  / {}", env!("CARGO_PKG_VERSION")), dim);
        let status = if live { "LIVE" } else { "OFFLINE" };
        let room = w.saturating_sub(head.width() + str_w(status) + 10).max(8);
        let path = if str_w(&path) > room {
            format!(
                "…{}",
                path.chars()
                    .rev()
                    .take(room - 1)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<String>()
            )
        } else {
            path
        };
        let right = format!("{path}  ·  {status}");
        let gap = w.saturating_sub(head.width() + str_w(&right) + 2).max(2);
        head.add(" ".repeat(gap), Style::default());
        head.add(right, dim);
        out.push(head);
        out.push(Line::styled(
            format!("  {}", "╌".repeat(w.saturating_sub(4))),
            Style::fg(t.border),
        ));
        out.push(Line::new());

        // Right column: wordmark, tagline, connection.
        let mut info: Vec<Line> = crate::mascot::wordmark(t);
        info.push(Line::new());
        info.push(Line::styled(
            "INTENT. COMPILE. EXECUTE.",
            Style::fg(t.accent),
        ));
        info.push(Line::styled("A clear path from thought to shipped.", dim));
        info.push(Line::new());
        if live {
            info.push(
                Line::styled("● ", Style::fg(t.ok))
                    .push("Connected", Style::default().bold())
                    .push(format!("  /  {provider}"), dim),
            );
        } else {
            info.push(
                Line::styled("● ", Style::fg(t.warn))
                    .push("Not connected", Style::default().bold())
                    .push("  /  run /onboard", dim),
            );
        }
        info.push(Line::styled(model, dim));

        if w >= crate::mascot::HERO_WIDTH + 34 {
            let hero = crate::mascot::hero(t);
            for (i, h) in hero.into_iter().enumerate() {
                let mut l = h;
                l.add("  ", Style::default());
                if i >= 1 {
                    if let Some(r) = info.get(i - 1) {
                        l.spans.extend(r.spans.clone());
                    }
                }
                out.push(l);
            }
        } else {
            for l in info {
                let mut row = Line::raw("  ");
                row.spans.extend(l.spans);
                out.push(row);
            }
        }
        out.push(Line::new());

        // Cards.
        let cards = [
            ("01 / CONNECT", "/onboard", "Choose your model"),
            ("02 / DIRECT", "/goal", "Give intent a direction"),
            ("03 / DISCOVER", "/help", "Explore your workspace"),
        ];
        if w >= 66 {
            let cw = (w.saturating_sub(4)) / 3 - 2;
            let border = Style::fg(t.border);
            let mut rows = vec![
                Line::raw("  "),
                Line::raw("  "),
                Line::raw("  "),
                Line::raw("  "),
                Line::raw("  "),
            ];
            for (title, cmd, detail) in cards {
                let inner = cw.saturating_sub(2);
                let top_fill = inner.saturating_sub(str_w(title));
                rows[0].add("╭", border);
                rows[0].add(title, dim);
                rows[0].add(format!("{}╮ ", "╌".repeat(top_fill)), border);
                for (k, (text, st)) in [
                    (cmd, Style::fg(t.accent).bold()),
                    (detail, dim),
                    ("", Style::default()),
                ]
                .into_iter()
                .enumerate()
                {
                    let text = render::truncate(text, inner);
                    rows[k + 1].add("┆", border);
                    rows[k + 1].add(text.clone(), st);
                    rows[k + 1].add(
                        format!("{}┆ ", " ".repeat(inner.saturating_sub(str_w(&text)))),
                        border,
                    );
                }
                rows[4].add(format!("╰{}╯ ", "╌".repeat(inner)), border);
            }
            out.extend(rows);
        } else {
            for (title, cmd, detail) in cards {
                out.push(
                    Line::styled(format!("  {title}  "), dim)
                        .push(cmd, Style::fg(t.accent).bold())
                        .push(format!("  {detail}"), dim),
                );
            }
        }
        out.push(Line::new());
        out
    }

    fn spinner_line(&self) -> Line {
        let t = &self.theme;
        let glyph = SPINNER[(self.tick as usize) % SPINNER.len()];
        let secs = self.run_started.elapsed().as_secs();
        let verb = if self.interrupting {
            "Interrupting"
        } else {
            self.verb
        };
        let mut l = Line::styled(format!("{glyph} "), Style::fg(t.accent));
        // Shimmer: a bright highlight sweeping across the verb.
        let chars: Vec<char> = format!("{verb}…").chars().collect();
        let pos = (self.tick as usize) % (chars.len() + 8);
        for (i, c) in chars.iter().enumerate() {
            let st = if i + 1 >= pos && i <= pos + 1 {
                Style::default().bold()
            } else {
                Style::fg(t.accent)
            };
            l.add(c.to_string(), st);
        }
        let mut meta = vec![format!("{secs}s")];
        if self.stream_chars > 0 {
            meta.push(format!(
                "↓ {} tokens",
                render::human_tokens((self.stream_chars / 4) as u64)
            ));
        }
        meta.push("esc to interrupt".into());
        l.add(format!(" ({})", meta.join(" · ")), Style::fg(t.dim));
        l
    }

    fn render_live(&mut self) -> anyhow::Result<()> {
        if self.transcript_mode {
            let l = vec![Line::styled(
                "  Showing detailed transcript · ctrl+r to toggle",
                Style::fg(self.theme.dim),
            )];
            self.scr.draw_live(&l, None)?;
            return Ok(());
        }
        let w = self.scr.cols();
        let t = self.theme;
        let mut lines: Vec<Line> = Vec::new();
        // Items still in flight.
        for it in &self.items[self.committed..] {
            lines.extend(self.render_item(it, false));
        }
        if self.show_thinking && !self.thinking_stream.trim().is_empty() {
            lines.push(Line::styled("✻ Thinking…", Style::fg(t.dim).italic()));
            let tl: Vec<&str> = self.thinking_stream.lines().collect();
            for l in tl.iter().skip(tl.len().saturating_sub(6)) {
                lines.extend(render::wrap(
                    &Line::styled(format!("  {l}"), Style::fg(t.dim).italic()),
                    w,
                    2,
                ));
            }
            lines.push(Line::new());
        }
        if !self.streaming.trim().is_empty() {
            let md = render::markdown(&self.streaming, w, "  ", &t);
            let budget = self.scr.height.saturating_sub(12).max(4);
            let skip = md.len().saturating_sub(budget);
            for (i, mut l) in md.into_iter().enumerate().skip(skip) {
                if i == 0 {
                    l.spans[0] = render::Span {
                        text: "⏺ ".into(),
                        style: Style::default(),
                    };
                }
                lines.push(l);
            }
            lines.push(Line::new());
        }
        if self.running && self.perm.is_none() {
            lines.push(self.spinner_line());
            if let Some(r) = &self.retry {
                lines.push(Line::styled(format!("  ⎿  {r}"), Style::fg(t.warn)));
            }
            if self.show_todos && !self.todos.is_empty() {
                lines.extend(render::todo_lines(&self.todos, &t, "  ⎿  "));
            }
            lines.push(Line::new());
        } else if !self.running
            && self.show_todos
            && !self.todos.is_empty()
            && self.perm.is_none()
            && self.picker.is_none()
        {
            // Keep an unfinished plan visible between turns.
            lines.extend(render::todo_lines(&self.todos, &t, "  ⎿  "));
            lines.push(Line::new());
        }
        for q in &self.queued {
            lines.push(Line::styled(
                format!("> {}", render::truncate(q, w.saturating_sub(4))),
                Style::fg(t.dim),
            ));
        }
        let mut cursor = None;
        if let Some(p) = &self.perm {
            lines.extend(crate::commands::permission_box(p, w, &t, &self.root));
        } else if let Some(p) = &self.picker {
            let (pl, cur) = crate::commands::picker_box(p, w, &t);
            if let Some((r, c)) = cur {
                cursor = Some((lines.len() + r, c));
            }
            lines.extend(pl);
        } else {
            let (bl, (r, c)) = self.input_box(w);
            cursor = Some((lines.len() + r, c));
            lines.extend(bl);
            lines.extend(self.footer(w));
        }
        self.scr.draw_live(&lines, cursor)?;
        Ok(())
    }

    /// "> YOUR NEXT MOVE" composer with a dashed frame and a hint row.
    fn input_box(&self, w: usize) -> (Vec<Line>, (usize, usize)) {
        let t = &self.theme;
        let (mode_char, color) = if self.input.text.starts_with('!') {
            ("!", t.bash)
        } else if self.input.text.starts_with('#') {
            ("#", t.memory)
        } else {
            (">", t.accent)
        };
        let frame = if mode_char != ">" {
            color
        } else if self.running {
            t.warn
        } else {
            t.border
        };
        let border = Style::fg(frame);
        let title = if mode_char == "!" {
            " ! BASH MODE ".to_string()
        } else if mode_char == "#" {
            " # MEMORIZE ".to_string()
        } else if self.running {
            " ◈ WORKING · your next instruction can wait here ".to_string()
        } else {
            " > YOUR NEXT MOVE ".to_string()
        };
        let inner = w.saturating_sub(6);
        let shown = if mode_char != ">" {
            &self.input.text[1..]
        } else {
            self.input.text.as_str()
        };
        let mut ed = Editor {
            text: shown.to_string(),
            cursor: self
                .input
                .cursor
                .saturating_sub(if mode_char != ">" { 1 } else { 0 }),
        };
        ed.cursor = ed.cursor.min(ed.text.len());
        let (rows, (cr, cc)) = ed.layout(inner, false);
        let fill = w.saturating_sub(3 + str_w(&title));
        let mut out = vec![Line::styled("╭╌", border)
            .push(title, Style::fg(t.dim))
            .push(format!("{}╮", "╌".repeat(fill)), border)];
        let placeholder = self.input.is_empty();
        for (i, r) in rows.iter().enumerate() {
            let pre = if i == 0 {
                format!("{mode_char} ")
            } else {
                "  ".into()
            };
            let (text, st) = if placeholder && i == 0 {
                let ph = if self.running && !self.queued.is_empty() {
                    "Press up to edit queued messages".to_string()
                } else if self.running {
                    "Type ahead — it will be sent when ICE finishes".to_string()
                } else {
                    "What would you like to build?  /help for commands".to_string()
                };
                (render::truncate(&ph, inner), Style::fg(t.dim))
            } else {
                (r.clone(), Style::default())
            };
            let pad = inner.saturating_sub(str_w(&text));
            out.push(
                Line::styled("┆ ", border)
                    .push(pre, Style::fg(color))
                    .push(text, st)
                    .push(" ".repeat(pad), Style::default())
                    .push(" ┆", border),
            );
        }
        let hint = "  Enter send   / commands   @ files   ! shell   Ctrl+L help   Esc interrupt";
        let hint = render::truncate(hint, inner + 2);
        out.push(
            Line::styled("┆ ", border)
                .push(hint.clone(), Style::fg(t.dim))
                .push(
                    " ".repeat((inner + 2).saturating_sub(str_w(&hint))),
                    Style::default(),
                )
                .push(" ┆", border),
        );
        out.push(Line::styled(format!("╰{}╯", "╌".repeat(w - 2)), border));
        (out, (1 + cr, 4 + cc))
    }

    /// Full-width context gauge: "context 1.2k/200k tok · 1% · theme ice".
    fn gauge(&self, w: usize) -> Line {
        let t = &self.theme;
        let win = if self.stats.window > 0 {
            self.stats.window
        } else {
            crate::model::ModelConfig::resolve(None)
                .map(|c| c.context_window())
                .unwrap_or(200_000)
        };
        let used = self.stats.context.min(win);
        let ratio = used as f64 / win.max(1) as f64;
        let mut label = format!(
            "context  {}/{} tok  ·  {:.0}%  ·  theme {}",
            used,
            win,
            ratio * 100.0,
            self.theme_name.id()
        );
        if let Some(c) = self.stats.cost {
            if c > 0.0 {
                label.push_str(&format!("  ·  ${c:.3}"));
            }
        }
        let label = render::truncate(&label, w);
        let lw = str_w(&label);
        let left = (w.saturating_sub(lw)) / 2;
        let text: Vec<char> = format!(
            "{}{}{}",
            " ".repeat(left),
            label,
            " ".repeat(w.saturating_sub(left + lw))
        )
        .chars()
        .collect();
        let filled = ((ratio * w as f64).round() as usize).min(w);
        let color = if ratio > 0.8 { t.warn } else { t.accent };
        let mut l = Line::new();
        let mut cur = String::new();
        let mut cur_on = false;
        let mut cells = 0;
        for c in text {
            let on = cells < filled;
            if on != cur_on && !cur.is_empty() {
                let bg = if cur_on { t.gauge } else { t.surface };
                l.add(std::mem::take(&mut cur), Style::fg(color).on(bg));
            }
            cur_on = on;
            cur.push(c);
            cells += render::char_w(c);
        }
        if !cur.is_empty() {
            l.add(
                cur,
                Style::fg(color).on(if cur_on { t.gauge } else { t.surface }),
            );
        }
        l
    }

    fn footer(&self, w: usize) -> Vec<Line> {
        let t = &self.theme;
        let mut out = vec![self.gauge(w)];
        if !self.sugg.is_empty() {
            let namew = self
                .sugg
                .iter()
                .map(|(n, _)| str_w(n))
                .max()
                .unwrap_or(10)
                .min(28);
            for (i, (name, desc)) in self.sugg.iter().enumerate() {
                let sel = i == self.sugg_idx;
                let st = if sel {
                    Style::fg(t.accent).bold()
                } else {
                    Style::fg(t.dim)
                };
                let pad = namew.saturating_sub(str_w(name));
                let l = Line::styled(format!("  {name}{}  ", " ".repeat(pad)), st).push(
                    render::truncate(desc, w.saturating_sub(namew + 6)),
                    if sel {
                        Style::default()
                    } else {
                        Style::fg(t.dim)
                    },
                );
                out.push(l);
            }
            return out;
        }
        if self.shortcuts {
            let cols = [
                [
                    "! for bash mode",
                    "/ for commands",
                    "@ for file paths",
                    "# to memorize",
                ],
                [
                    "double tap esc to rewind",
                    "shift + tab to cycle modes",
                    "\\⏎ or alt+⏎ for newline",
                    "esc to interrupt",
                ],
                [
                    "ctrl + r for verbose output",
                    "ctrl + t to toggle todos",
                    "ctrl + l for this help",
                    "ctrl + c to exit",
                ],
            ];
            let cw = (w / 3).max(20);
            for r in 0..4 {
                let mut l = Line::raw("  ");
                for c in cols.iter() {
                    let s = render::truncate(c[r], cw - 2);
                    let pad = cw.saturating_sub(str_w(&s));
                    l.add(format!("{s}{}", " ".repeat(pad)), Style::fg(t.dim));
                }
                out.push(l);
            }
            return out;
        }
        let dot = |c| Style::fg(c);
        let mut l = if self.ctrl_c_at.is_some() {
            Line::styled(" ● ", dot(t.warn)).push("Press Ctrl-C again to exit", Style::fg(t.dim))
        } else if self
            .esc_at
            .map(|a| a.elapsed() < Duration::from_millis(900))
            .unwrap_or(false)
            && !self.input.is_empty()
        {
            Line::styled(" ● ", dot(t.warn)).push("Esc again to clear", Style::fg(t.dim))
        } else if let Some((f, _)) = &self.flash {
            Line::styled(" ● ", dot(t.accent)).push(f.clone(), Style::fg(t.dim))
        } else if self.running {
            Line::styled(" ◐ ", dot(t.accent)).push("Working…  esc to interrupt", Style::fg(t.dim))
        } else if !self.connected() {
            Line::styled(" ● ", dot(t.ok))
                .push("Ready when you are.  ", Style::fg(t.dim))
                .push("/onboard", Style::fg(t.accent))
                .push(" to connect a provider", Style::fg(t.dim))
        } else if let Some(g) = &self.goal {
            Line::styled(" ◎ ", dot(t.accent)).push(
                format!("Goal: {}", render::truncate(g, w.saturating_sub(30))),
                Style::fg(t.dim),
            )
        } else {
            Line::styled(" ● ", dot(t.ok))
                .push("Ready when you are.  ", Style::fg(t.dim))
                .push("? for shortcuts", Style::fg(t.dim))
        };
        match self.mode {
            Mode::AcceptEdits => l.add(
                "   ⏵⏵ accept edits on (shift+tab to cycle)",
                Style::fg(t.edits),
            ),
            Mode::Plan => l.add("   ⏸ plan mode on (shift+tab to cycle)", Style::fg(t.plan)),
            Mode::BypassPermissions => l.add("   ⏵⏵ bypass permissions on", Style::fg(t.err)),
            Mode::Default => {}
        }
        let mut right = String::new();
        if self.stats.window > 0 && self.stats.context > 0 {
            let limit = self.stats.window as f64 * 0.92;
            let left_pct = ((1.0 - self.stats.context as f64 / limit) * 100.0).clamp(0.0, 100.0);
            if left_pct < 20.0 {
                right = format!("Context left until auto-compact: {left_pct:.0}%");
            }
        }
        if right.is_empty() {
            if let Some(n) = &self.update_note {
                right = n.clone();
            }
        }
        let gap = w.saturating_sub(l.width() + str_w(&right) + 1);
        if !right.is_empty() && gap > 1 {
            l.add(" ".repeat(gap), Style::default());
            l.add(right, Style::fg(t.warn));
        }
        out.push(l);
        out
    }

    // ─────────────────────────── input ───────────────────────────

    fn on_terminal_event(&mut self, ev: CEvent) {
        match ev {
            CEvent::Key(k) if k.kind != KeyEventKind::Release => self.on_key(k),
            CEvent::Paste(s) => self.on_paste(s),
            CEvent::Resize(_, _) => {
                self.scr.refresh_size();
                let saved = self.transcript_mode;
                self.redraw_all();
                self.transcript_mode = saved;
            }
            _ => {}
        }
    }

    fn on_paste(&mut self, s: String) {
        if let Some(p) = &mut self.picker {
            if let Some(e) = &mut p.entry {
                e.insert_str(s.trim());
            }
            return;
        }
        let lines = s.lines().count();
        if s.len() > 800 || lines > 10 {
            self.pastes.push(s);
            let tag = format!("[Pasted text #{} +{} lines]", self.pastes.len(), lines);
            self.input.insert_str(&tag);
        } else {
            self.input.insert_str(&s);
        }
        self.update_suggestions();
    }

    fn on_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        // Global keys.
        if ctrl && k.code == KeyCode::Char('c') {
            return self.on_ctrl_c();
        }
        if ctrl && k.code == KeyCode::Char('r') {
            self.transcript_mode = !self.transcript_mode;
            self.redraw_all();
            return;
        }
        if self.transcript_mode {
            if k.code == KeyCode::Esc || k.code == KeyCode::Char('q') {
                self.transcript_mode = false;
                self.redraw_all();
            }
            return;
        }
        if ctrl && k.code == KeyCode::Char('l') {
            self.shortcuts = !self.shortcuts;
            return;
        }
        if ctrl && k.code == KeyCode::Char('t') {
            self.show_todos = !self.show_todos;
            return;
        }
        if self.perm.is_some() {
            return self.on_perm_key(k);
        }
        if self.picker.is_some() {
            return crate::commands::on_picker_key(self, k);
        }
        if k.code == KeyCode::BackTab
            || (k.code == KeyCode::Tab && k.modifiers.contains(KeyModifiers::SHIFT))
        {
            self.mode = self.mode.cycle(self.bypass_available);
            self.send(Cmd::Mode(self.mode));
            return;
        }
        if k.code == KeyCode::Esc {
            return self.on_esc();
        }
        self.esc_at = None;
        match k.code {
            KeyCode::Enter => {
                let newline = alt || k.modifiers.contains(KeyModifiers::SHIFT);
                if newline {
                    self.input.insert('\n');
                } else if self.input.text[..self.input.cursor].ends_with('\\') {
                    self.input.backspace();
                    self.input.insert('\n');
                } else if !self.sugg.is_empty() && self.accept_suggestion(true) {
                    // accepted a command and submitted
                } else {
                    self.submit_input();
                }
            }
            KeyCode::Char('j') if ctrl => self.input.insert('\n'),
            KeyCode::Tab => {
                if !self.sugg.is_empty() {
                    self.accept_suggestion(false);
                }
            }
            KeyCode::Up => {
                if !self.sugg.is_empty() {
                    self.sugg_idx = (self.sugg_idx + self.sugg.len() - 1) % self.sugg.len();
                } else if self.running && self.input.is_empty() && !self.queued.is_empty() {
                    let q = std::mem::take(&mut self.queued).join("\n");
                    self.input.set(&q);
                } else if !self.input.up() {
                    self.history_prev();
                }
            }
            KeyCode::Down => {
                if !self.sugg.is_empty() {
                    self.sugg_idx = (self.sugg_idx + 1) % self.sugg.len();
                } else if !self.input.down() {
                    self.history_next();
                }
            }
            KeyCode::Left if ctrl || alt => self.input.word_left(),
            KeyCode::Right if ctrl || alt => self.input.word_right(),
            KeyCode::Left => self.input.left(),
            KeyCode::Right => self.input.right(),
            KeyCode::Home => self.input.home(),
            KeyCode::End => self.input.end(),
            KeyCode::Backspace if ctrl || alt => self.input.delete_word_back(),
            KeyCode::Backspace => self.input.backspace(),
            KeyCode::Delete => self.input.delete(),
            KeyCode::Char('a') if ctrl => self.input.home(),
            KeyCode::Char('e') if ctrl => self.input.end(),
            KeyCode::Char('b') if ctrl => self.input.left(),
            KeyCode::Char('f') if ctrl => self.input.right(),
            KeyCode::Char('u') if ctrl => self.input.kill_to_start(),
            KeyCode::Char('k') if ctrl => self.input.kill_to_end(),
            KeyCode::Char('w') if ctrl => self.input.delete_word_back(),
            KeyCode::Char('h') if ctrl => self.input.backspace(),
            KeyCode::Char('d') if ctrl => {
                if self.input.is_empty() {
                    self.quit = true;
                } else {
                    self.input.delete();
                }
            }
            KeyCode::Char('b') if alt => self.input.word_left(),
            KeyCode::Char('f') if alt => self.input.word_right(),
            KeyCode::Char('?') if self.input.is_empty() && !ctrl => {
                self.shortcuts = !self.shortcuts
            }
            KeyCode::Char(c) if !ctrl => {
                self.shortcuts = false;
                self.input.insert(c);
            }
            _ => {}
        }
        self.update_suggestions();
    }

    fn on_ctrl_c(&mut self) {
        if self.perm.is_some() {
            if let Some(p) = self.perm.take() {
                let _ = p.req.reply.send(PermReply::Deny(None));
            }
            self.cancel.store(true, Ordering::Relaxed);
            self.interrupting = true;
            return;
        }
        if self.picker.is_some() {
            crate::commands::close_picker(self);
            return;
        }
        if self.running {
            self.cancel.store(true, Ordering::Relaxed);
            self.interrupting = true;
            return;
        }
        if !self.input.is_empty() {
            self.input.take();
            self.update_suggestions();
            return;
        }
        match self.ctrl_c_at {
            Some(at) if at.elapsed() < Duration::from_millis(1500) => self.quit = true,
            _ => self.ctrl_c_at = Some(Instant::now()),
        }
    }

    fn on_esc(&mut self) {
        if !self.sugg.is_empty() {
            self.sugg.clear();
            self.sugg_dismissed = true;
            return;
        }
        if self.shortcuts {
            self.shortcuts = false;
            return;
        }
        if self.running {
            self.cancel.store(true, Ordering::Relaxed);
            self.interrupting = true;
            return;
        }
        let double = self
            .esc_at
            .map(|a| a.elapsed() < Duration::from_millis(900))
            .unwrap_or(false);
        if double {
            self.esc_at = None;
            if self.input.is_empty() {
                crate::commands::open_rewind(self);
            } else {
                self.input.take();
            }
        } else {
            self.esc_at = Some(Instant::now());
        }
    }

    fn on_perm_key(&mut self, k: KeyEvent) {
        let Some(p) = &mut self.perm else { return };
        let opts = crate::commands::perm_options(p);
        let n = opts.len();
        let reply = match k.code {
            KeyCode::Up | KeyCode::Char('k') => {
                p.idx = (p.idx + n - 1) % n;
                None
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                p.idx = (p.idx + 1) % n;
                None
            }
            KeyCode::Char(c @ '1'..='9') => {
                opts.get(c as usize - '1' as usize).map(|o| o.1.clone())
            }
            KeyCode::Char('y') => Some(opts[0].1.clone()),
            KeyCode::Char('n') | KeyCode::Esc => Some(PermReply::Deny(None)),
            KeyCode::Enter => Some(opts[p.idx.min(n - 1)].1.clone()),
            KeyCode::BackTab if p.req.always_label.is_some() => Some(PermReply::AllowAlways),
            _ => None,
        };
        if let Some(r) = reply {
            let p = self.perm.take().unwrap();
            let _ = p.req.reply.send(r);
        }
    }

    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let i = match self.hist_idx {
            None => {
                self.hist_draft = self.input.text.clone();
                self.history.len() - 1
            }
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.hist_idx = Some(i);
        self.input.set(&self.history[i]);
    }

    fn history_next(&mut self) {
        match self.hist_idx {
            None => {}
            Some(i) if i + 1 >= self.history.len() => {
                self.hist_idx = None;
                let d = std::mem::take(&mut self.hist_draft);
                self.input.set(&d);
            }
            Some(i) => {
                self.hist_idx = Some(i + 1);
                self.input.set(&self.history[i + 1]);
            }
        }
    }

    // ─────────────────────────── suggestions ───────────────────────────

    pub(crate) fn update_suggestions(&mut self) {
        let text = self.input.text.clone();
        self.sugg.clear();
        if self.sugg_dismissed {
            if text.is_empty() {
                self.sugg_dismissed = false;
            }
            return;
        }
        if let Some(q) = text.strip_prefix('/') {
            if !q.contains(char::is_whitespace) {
                let q = q.to_ascii_lowercase();
                let mut list: Vec<(String, String)> = crate::commands::all_commands(&self.root)
                    .into_iter()
                    .filter(|(n, _)| n.starts_with(&q) || (q.len() >= 2 && n.contains(&q)))
                    .map(|(n, d)| (format!("/{n}"), d))
                    .collect();
                list.sort_by_key(|(n, _)| (!n[1..].starts_with(&q), n.len()));
                list.truncate(8);
                self.sugg = list;
            }
        } else {
            let (_, tok) = self.input.token_at_cursor();
            if let Some(q) = tok.strip_prefix('@') {
                let q = q.to_ascii_lowercase();
                let files = self.project_files();
                let mut hits: Vec<(String, String)> = files
                    .iter()
                    .filter(|f| q.is_empty() || f.to_ascii_lowercase().contains(&q))
                    .take(200)
                    .map(|f| (f.clone(), String::new()))
                    .collect();
                hits.sort_by_key(|(f, _)| (!f.to_ascii_lowercase().starts_with(&q), f.len()));
                hits.truncate(8);
                self.sugg = hits
                    .into_iter()
                    .map(|(f, d)| (format!("@{f}"), d))
                    .collect();
            }
        }
        if self.sugg_idx >= self.sugg.len() {
            self.sugg_idx = 0;
        }
    }

    fn project_files(&mut self) -> Vec<String> {
        if let Some((at, files)) = &self.files_cache {
            if at.elapsed() < Duration::from_secs(30) {
                return files.clone();
            }
        }
        let mut files = Vec::new();
        let walk = ignore::WalkBuilder::new(&self.root)
            .hidden(true)
            .git_ignore(true)
            .require_git(false)
            .max_depth(Some(8))
            .build();
        for e in walk.flatten().take(5000) {
            if e.depth() == 0 {
                continue;
            }
            if let Ok(rel) = e.path().strip_prefix(&self.root) {
                let mut s = rel.display().to_string();
                if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    s.push('/');
                }
                files.push(s);
            }
        }
        self.files_cache = Some((Instant::now(), files.clone()));
        files
    }

    /// Apply the highlighted suggestion. With `submit`, a completed slash
    /// command runs immediately (Enter).
    fn accept_suggestion(&mut self, submit: bool) -> bool {
        let Some((choice, _)) = self.sugg.get(self.sugg_idx).cloned() else {
            return false;
        };
        if choice.starts_with('/') {
            let typed = self.input.text.trim().to_string();
            self.input.set(&format!("{choice} "));
            self.sugg.clear();
            if submit && (typed == choice || typed.len() > 1) {
                self.input.set(choice.trim());
                self.submit_input();
            }
            return true;
        }
        let (start, _) = self.input.token_at_cursor();
        let end = self.input.cursor;
        let with = if choice.ends_with('/') {
            choice.clone()
        } else {
            format!("{choice} ")
        };
        self.input.replace_range(start, end, &with);
        self.sugg.clear();
        if choice.ends_with('/') {
            self.update_suggestions();
        }
        true
    }

    // ─────────────────────────── submit ───────────────────────────

    fn submit_input(&mut self) {
        let raw = self.input.take();
        self.hist_idx = None;
        self.sugg.clear();
        self.sugg_dismissed = false;
        self.shortcuts = false;
        if raw.trim().is_empty() {
            return;
        }
        // Expand pasted-text placeholders.
        let mut text = raw.clone();
        for (i, p) in self.pastes.iter().enumerate() {
            let re =
                regex::Regex::new(&format!(r"\[Pasted text #{} \+\d+ lines\]", i + 1)).unwrap();
            text = re.replace_all(&text, regex::NoExpand(p)).into_owned();
        }
        self.pastes.clear();
        push_history(&mut self.history, &raw);
        self.submit_text(text);
    }

    pub(crate) fn submit_text(&mut self, text: String) {
        let trimmed = text.trim().to_string();
        if trimmed.starts_with('/') && !trimmed.starts_with("//") {
            crate::commands::slash(self, &trimmed);
            return;
        }
        if let Some(cmd) = trimmed.strip_prefix('!') {
            if self.running {
                self.queued.push(text);
                return;
            }
            self.running = true;
            self.run_started = Instant::now();
            self.verb = "Running";
            self.stream_chars = 0;
            self.send(Cmd::Bash(cmd.trim().to_string()));
            return;
        }
        if let Some(mem) = trimmed.strip_prefix('#') {
            crate::commands::open_memory_picker(self, mem.trim().to_string());
            return;
        }
        if self.running {
            self.queued.push(text);
            return;
        }
        self.start_turn(trimmed.clone(), trimmed);
    }

    /// Show `display` as the user line and send `prompt` to the model.
    pub(crate) fn start_turn(&mut self, display: String, prompt: String) {
        self.items.push(Item::User(display));
        self.running = true;
        self.interrupting = false;
        self.run_started = Instant::now();
        self.verb = VERBS[(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0) as usize)
            % VERBS.len()];
        self.stream_chars = 0;
        self.cancel.store(false, Ordering::Relaxed);
        self.send(Cmd::Submit(prompt));
    }
}

fn install_panic_hook(kb: bool) {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if kb {
            let _ = crossterm::execute!(std::io::stdout(), cev::PopKeyboardEnhancementFlags);
        }
        let _ = crossterm::execute!(
            std::io::stdout(),
            cev::DisableBracketedPaste,
            crossterm::cursor::Show
        );
        let _ = crossterm::terminal::disable_raw_mode();
        eprintln!();
        prev(info);
    }));
}

fn history_path() -> PathBuf {
    crate::settings::user_dir().join("history.jsonl")
}

fn load_history() -> Vec<String> {
    std::fs::read_to_string(history_path())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<String>(l).ok())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .take(500)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn push_history(h: &mut Vec<String>, entry: &str) {
    if h.last().map(|l| l == entry).unwrap_or(false) {
        return;
    }
    h.push(entry.to_string());
    let p = history_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
    {
        use std::io::Write;
        let _ = writeln!(f, "{}", serde_json::to_string(entry).unwrap_or_default());
    }
}

/// Rebuild display items from stored messages (for --resume).
pub fn items_from_messages(msgs: &[Message]) -> Vec<Item> {
    let mut items = Vec::new();
    let mut results: std::collections::HashMap<String, (String, bool)> =
        std::collections::HashMap::new();
    for m in msgs {
        for b in &m.content {
            if let Block::ToolResult {
                tool_use_id,
                content,
                is_error,
            } = b
            {
                results.insert(tool_use_id.clone(), (content.clone(), *is_error));
            }
        }
    }
    for m in msgs {
        for b in &m.content {
            match (m.role, b) {
                (Role::User, Block::Text { text }) if engine::is_user_prompt(text) => {
                    items.push(Item::User(text.clone()))
                }
                (Role::Assistant, Block::Text { text }) if !text.trim().is_empty() => {
                    items.push(Item::Assistant(text.clone()))
                }
                (Role::Assistant, Block::ToolUse { id, name, input }) => {
                    let (content, is_error) = results.get(id).cloned().unwrap_or_default();
                    let first = content.lines().next().unwrap_or("").to_string();
                    let summary = if is_error {
                        first.clone()
                    } else {
                        format!(
                            "{} line{} of output",
                            content.lines().count(),
                            if content.lines().count() == 1 {
                                ""
                            } else {
                                "s"
                            }
                        )
                    };
                    items.push(Item::Tool {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                        output: Some(ToolOutput {
                            content,
                            is_error,
                            display: Display::Lines {
                                summary,
                                body: vec![],
                            },
                        }),
                        steps: vec![],
                    });
                }
                _ => {}
            }
        }
    }
    items
}
