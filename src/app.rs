use crate::goal::{self, DurableGoal};
use crate::harness::{self, Event};
use crate::ir::Action;
use crate::providers::{self, IceConfig, PROVIDERS};
use crate::theme::{self, Theme, ThemeName};
use crate::todos::{self, Todo};
use std::path::PathBuf;
use crate::update::{self, UpdateEvent};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Onboard,
    Welcome,
    Session,
}

#[derive(Clone)]
pub enum MsgKind {
    User,
    System,
    Thinking,
    Burst,
    Step { ok: bool },
    Assert { ok: bool },
    Assistant,
    Agent,
    Loop,
    Goal,
    Error,
}

#[derive(Clone)]
pub struct Msg {
    pub kind: MsgKind,
    pub title: String,
    pub body: String,
}

pub struct App {
    pub screen: Screen,
    pub theme_name: ThemeName,
    pub theme: Theme,
    pub input: String,
    pub cursor: usize,
    pub messages: Vec<Msg>,
    pub scroll: u16,
    pub running: bool,
    pub should_quit: bool,
    pub help: bool,
    pub yolo: bool,
    pub demo: bool,
    pub model: String,
    pub root: PathBuf,
    pub session: String,
    pub turns: u32,
    pub bursts: u32,
    pub steps_ok: u32,
    pub steps_fail: u32,
    pub status: String,
    pub tick: u64,
    pub rx: Option<Receiver<Event>>,
    pub last_delta: Option<String>,
    pub goal_text: Option<String>,
    pub loop_pass: u32,
    pub agents_alive: u32,
    pub agents_done: u32,
    pub provider: String,
    pub onboard_step: u8,
    pub onboard_idx: usize,
    pub models: Vec<String>,
    pub model_idx: usize,
    pub show_thinking: bool,
    pub input_focused: bool,
    pub context_used: u32,
    pub context_window: u32,
    pub todos: Vec<Todo>,
    pub update_rx: Option<Receiver<UpdateEvent>>,
    pub update_tx: Option<Sender<UpdateEvent>>,
    pub update_note: Option<String>,
    pub update_checking: bool,
}

impl App {
    pub fn new(root: PathBuf, demo: bool) -> Self {
        let cfg = IceConfig::load();
        cfg.apply_env();
        let model = cfg.model.clone();
        let has_key = crate::llm::Llm::from_env().is_some();
        let goal_text = DurableGoal::load(&root).map(|g| g.text);
        let onboard = !cfg.onboarded;
        Self {
            screen: if onboard {
                Screen::Onboard
            } else {
                Screen::Welcome
            },
            theme_name: ThemeName::Ice,
            theme: Theme::ice(),
            input: String::new(),
            cursor: 0,
            messages: Vec::new(),
            scroll: 0,
            running: false,
            should_quit: false,
            help: false,
            yolo: false,
            demo: demo || !has_key,
            model: model.clone(),
            root: root.clone(),
            session: session_id(),
            turns: 0,
            bursts: 0,
            steps_ok: 0,
            steps_fail: 0,
            status: if has_key && !demo {
                "ready · ICE loop idle".into()
            } else {
                "demo mode · set XAI_API_KEY for live model".into()
            },
            tick: 0,
            rx: None,
            last_delta: None,
            goal_text,
            loop_pass: 0,
            agents_alive: 0,
            agents_done: 0,
            provider: cfg.provider.clone(),
            onboard_step: 0,
            onboard_idx: PROVIDERS
                .iter()
                .position(|p| p.id == cfg.provider)
                .unwrap_or(0),
            models: Vec::new(),
            model_idx: 0,
            show_thinking: true,
            input_focused: true,
            context_used: 0,
            context_window: theme::context_window_for(&model),
            todos: todos::load(&root),
            update_rx: None,
            update_tx: None,
            update_note: None,
            update_checking: false,
        }
    }

    /// Drain background-updater events into a banner + system messages.
    pub fn pump_updates(&mut self) {
        let Some(rx) = self.update_rx.take() else {
            return;
        };
        while let Ok(ev) = rx.try_recv() {
            match ev {
                UpdateEvent::Checking => {
                    self.update_checking = true;
                }
                UpdateEvent::UpToDate(v) => {
                    self.update_checking = false;
                    self.update_note = Some(format!("up to date · v{v}"));
                }
                UpdateEvent::Available { version } => {
                    self.update_checking = false;
                    self.update_note = Some(format!("update {version} available · downloading…"));
                }
                UpdateEvent::Downloading { version } => {
                    self.update_note = Some(format!("downloading {version}…"));
                }
                UpdateEvent::Installed { version } => {
                    self.update_checking = false;
                    self.update_note =
                        Some(format!("✓ updated to {version} — restart ICE to apply"));
                    self.messages.push(Msg {
                        kind: MsgKind::System,
                        title: "update".into(),
                        body: format!(
                            "ICE {version} was installed in the background. Restart to run the new version."
                        ),
                    });
                }
                UpdateEvent::Failed(e) => {
                    self.update_checking = false;
                    self.update_note = Some(format!("update check failed · {e}"));
                }
            }
        }
        self.update_rx = Some(rx);
    }

    fn cmd_update(&mut self) {
        let Some(tx) = self.update_tx.clone() else {
            self.push_sys("auto-update is disabled (ICE_NO_AUTO_UPDATE set).");
            return;
        };
        self.push_sys(&format!(
            "checking for updates (current v{})…",
            update::current_version()
        ));
        self.update_checking = true;
        std::thread::spawn(move || {
            let send = |e: UpdateEvent| {
                let _ = tx.send(e);
            };
            if let Err(e) = update::check_and_install(&send) {
                let _ = tx.send(UpdateEvent::Failed(e.to_string()));
            }
        });
    }

    pub fn skip_onboard(&mut self) {
        let mut cfg = IceConfig::load();
        cfg.onboarded = true;
        let _ = cfg.save();
        self.screen = Screen::Welcome;
        self.status = "onboard skipped · /onboard to reopen".into();
    }

    pub fn on_tab(&mut self) {
        match self.screen {
            Screen::Onboard => self.onboard_next(),
            Screen::Welcome => self.screen = Screen::Session,
            Screen::Session => self.input_focused = !self.input_focused,
        }
    }

    pub fn onboard_next(&mut self) {
        match self.onboard_step {
            0 => {
                let p = PROVIDERS[self.onboard_idx.min(PROVIDERS.len() - 1)];
                self.provider = p.id.into();
                self.model = p.default_model.into();
                let mut cfg = IceConfig::load();
                cfg.provider = p.id.into();
                cfg.model = p.default_model.into();
                cfg.base_url = Some(p.base_url.into());
                cfg.apply_env();
                let _ = cfg.save();
                self.onboard_step = 1;
                self.status = format!("provider {} · paste key or enter if env is set", p.id);
            }
            1 => {
                let p = PROVIDERS[self.onboard_idx.min(PROVIDERS.len() - 1)];
                if !self.input.trim().is_empty() {
                    std::env::set_var(p.key_envs[0], self.input.trim());
                    self.input.clear();
                    self.cursor = 0;
                }
                self.models =
                    providers::list_models(&p).unwrap_or_else(|_| vec![p.default_model.into()]);
                self.model_idx = self
                    .models
                    .iter()
                    .position(|m| m == &self.model)
                    .unwrap_or(0);
                self.onboard_step = 2;
                self.status = format!("{} models discovered", self.models.len());
            }
            2 => {
                if let Some(m) = self.models.get(self.model_idx) {
                    self.model = m.clone();
                }
                let mut cfg = IceConfig::load();
                cfg.provider = self.provider.clone();
                cfg.model = self.model.clone();
                cfg.onboarded = true;
                cfg.apply_env();
                let _ = cfg.save();
                self.demo = crate::llm::Llm::from_env().is_none();
                self.onboard_step = 3;
                self.screen = Screen::Welcome;
                self.status = format!("ready · {} / {}", self.provider, self.model);
            }
            _ => self.screen = Screen::Welcome,
        }
    }

    pub fn toggle_theme(&mut self) {
        self.theme_name = self.theme_name.next();
        self.theme = self.theme_name.palette();
        self.status = format!("theme {}", self.theme_name.id());
    }

    pub fn set_theme(&mut self, name: &str) {
        if let Some(n) = ThemeName::parse(name) {
            self.theme_name = n;
            self.theme = n.palette();
            self.status = format!("theme {}", n.id());
        } else {
            self.push_sys("themes: ice (default) · groknight · frost · ember · mono");
        }
    }

    pub fn insert(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = self.input[..self.cursor]
            .char_indices()
            .last()
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.input.replace_range(prev..self.cursor, "");
        self.cursor = prev;
    }

    pub fn move_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor = self.input[..self.cursor]
            .char_indices()
            .last()
            .map(|(i, _)| i)
            .unwrap_or(0);
    }

    pub fn move_right(&mut self) {
        if self.cursor >= self.input.len() {
            return;
        }
        let n = self.input[self.cursor..]
            .chars()
            .next()
            .map(|c| c.len_utf8())
            .unwrap_or(0);
        self.cursor += n;
    }

    pub fn submit(&mut self) {
        if self.screen == Screen::Onboard {
            self.onboard_next();
            return;
        }
        let text = self.input.trim().to_string();
        if text.is_empty() || self.running {
            return;
        }
        self.input.clear();
        self.cursor = 0;
        if text.starts_with('/') {
            self.slash(&text);
            return;
        }
        if self.screen == Screen::Welcome {
            self.screen = Screen::Session;
        }
        self.messages.push(Msg {
            kind: MsgKind::User,
            title: "you".into(),
            body: text.clone(),
        });
        if is_conversational(&text) {
            self.start_chat(text);
        } else {
            self.start_job(text, false, None);
        }
    }

    fn slash(&mut self, cmd: &str) {
        let (name, arg) = match cmd.split_once(char::is_whitespace) {
            Some((n, a)) => (n, a.trim()),
            None => (cmd, ""),
        };
        match name {
            "/exit" | "/quit" => self.should_quit = true,
            "/help" | "/?" => self.help = !self.help,
            "/new" | "/clear" => {
                self.messages.clear();
                self.turns = 0;
                self.bursts = 0;
                self.steps_ok = 0;
                self.steps_fail = 0;
                self.last_delta = None;
                self.goal_text = None;
                self.loop_pass = 0;
                self.agents_alive = 0;
                self.agents_done = 0;
                DurableGoal::clear(&self.root);
                self.session = session_id();
                self.screen = Screen::Welcome;
                self.status = "new session".into();
            }
            "/theme" | "/t" => {
                if arg.is_empty() {
                    self.toggle_theme();
                    self.push_sys(&format!("theme {}", self.theme_name.id()));
                } else {
                    self.set_theme(arg);
                }
            }
            "/skills" | "/skill" => {
                if arg.is_empty() {
                    self.push_sys(&crate::skills::catalog(&self.root));
                } else if let Some(s) = crate::skills::load_named(&self.root, arg) {
                    self.push_sys(&format!("skill {}\n{}", s.name, s.body));
                } else {
                    self.push_sys(&format!("unknown skill {arg}"));
                }
            }
            "/mcp" => self.push_sys(&crate::mcp::catalog(&self.root)),
            "/todo" => {
                if arg.is_empty() {
                    self.todos = todos::load(&self.root);
                    self.push_sys(&todos::render(&self.root));
                } else if let Some(rest) = arg.strip_prefix("done ") {
                    todos::complete(&self.root, rest.trim());
                    self.todos = todos::load(&self.root);
                    self.push_sys(&todos::render(&self.root));
                } else {
                    todos::add(&self.root, arg);
                    self.todos = todos::load(&self.root);
                    self.push_sys(&todos::render(&self.root));
                }
            }
            "/yolo" => {
                self.yolo = !self.yolo;
                self.status = if self.yolo {
                    "yolo on · auto-approve bursts".into()
                } else {
                    "yolo off".into()
                };
            }
            "/demo" => {
                self.demo = true;
                self.status = "demo mode on".into();
            }
            "/live" => {
                if crate::llm::Llm::from_env().is_some() {
                    self.demo = false;
                    self.status = "live model on".into();
                } else {
                    self.push_sys("no API key in env");
                }
            }
            "/status" | "/info" => {
                self.push_sys(&format!(
                    "session {} · model {} · demo {} · goal {} · loop {} · agents {}/{} · cwd {}",
                    self.session,
                    self.model,
                    self.demo,
                    self.goal_text.as_deref().unwrap_or("(none)"),
                    self.loop_pass,
                    self.agents_done,
                    self.agents_alive + self.agents_done,
                    self.root.display()
                ));
                self.screen = Screen::Session;
            }
            "/home" | "/welcome" => self.screen = Screen::Welcome,
            "/onboard" => {
                self.screen = Screen::Onboard;
                self.onboard_step = 0;
            }
            "/init" => match crate::init::ensure(&self.root) {
                Ok(p) => self.push_sys(&format!(
                    "{}\nwrote {}",
                    crate::init::describe(&self.root),
                    p.display()
                )),
                Err(e) => self.push_sys(&format!("init failed: {e}")),
            },
            "/update" | "/upgrade" => self.cmd_update(),
            "/providers" | "/provider" => self.cmd_providers(arg),
            "/models" => self.cmd_models(),
            "/clis" | "/cli" => self.cmd_clis(),
            "/think" | "/thinking" => {
                self.show_thinking = !self.show_thinking;
                self.push_sys(&format!(
                    "thinking print {}",
                    if self.show_thinking { "on" } else { "off" }
                ));
            }
            "/model" => {
                if !arg.is_empty() {
                    self.model = arg.to_string();
                    std::env::set_var("ICE_MODEL", arg);
                }
                self.push_sys(&format!("model = {}", self.model));
            }
            "/goal" => self.cmd_goal(arg),
            "/loop" => self.cmd_loop(arg),
            "/agent" | "/spawn" => self.cmd_agent(arg),
            "/agents" => self.cmd_agents(),
            "/cwd" => {
                if !arg.is_empty() {
                    let p = PathBuf::from(arg);
                    if p.is_dir() {
                        self.root = p;
                    }
                }
                self.push_sys(&format!("cwd = {}", self.root.display()));
            }
            other => self.push_sys(&format!("unknown command {other} · /help")),
        }
    }

    fn cmd_goal(&mut self, arg: &str) {
        if arg.is_empty() || arg == "show" {
            match DurableGoal::load(&self.root) {
                Some(g) => self.push_sys(&format!(
                    "goal: {}\nstatus: {}\nmax loops: {}\nprogress:\n{}",
                    g.text,
                    if g.open { "open" } else { "done" },
                    g.max_loops,
                    DurableGoal::read_progress(&self.root)
                )),
                None => self.push_sys("no durable goal. /goal <text> until file_exists .ice/DONE"),
            }
            return;
        }
        if arg == "clear" || arg == "stop" {
            DurableGoal::clear(&self.root);
            self.goal_text = None;
            self.push_sys("goal cleared");
            return;
        }
        let (text, done, max) = goal::parse_goal_arg(arg);
        let g = DurableGoal::new(&text, done, max);
        if let Err(e) = g.save(&self.root) {
            self.push_sys(&format!("could not persist goal: {e}"));
            return;
        }
        self.goal_text = Some(text.clone());
        self.messages.push(Msg {
            kind: MsgKind::Goal,
            title: "goal".into(),
            body: format!("{text}\nloop budget {max} · marker .ice/DONE"),
        });
        self.start_job(text, true, Some(g));
    }

    fn cmd_loop(&mut self, arg: &str) {
        if arg == "stop" {
            self.push_sys("loop will stop after the current burst");
            return;
        }
        let extra = arg
            .strip_prefix("until ")
            .or_else(|| arg.strip_prefix("n="))
            .unwrap_or(arg);
        let (mut text, done, mut max) = if extra.is_empty() {
            (
                self.goal_text
                    .clone()
                    .unwrap_or_else(|| "continue the open ICE goal".into()),
                vec![crate::ir::Assert::FileExists {
                    path: ".ice/DONE".into(),
                }],
                8u32,
            )
        } else if extra.chars().all(|c| c.is_ascii_digit()) {
            (
                self.goal_text
                    .clone()
                    .unwrap_or_else(|| "continue the open ICE goal".into()),
                vec![crate::ir::Assert::FileExists {
                    path: ".ice/DONE".into(),
                }],
                extra.parse().unwrap_or(8),
            )
        } else {
            goal::parse_goal_arg(extra)
        };
        if let Some(existing) = &self.goal_text {
            if text == "continue the open ICE goal" {
                text = existing.clone();
            }
        }
        if max == 0 {
            max = 8;
        }
        let g = DurableGoal::new(&text, done, max);
        let _ = g.save(&self.root);
        self.goal_text = Some(text.clone());
        self.messages.push(Msg {
            kind: MsgKind::Loop,
            title: "loop".into(),
            body: format!("outer loop ×{max}\n{text}"),
        });
        self.start_job(text, true, Some(g));
    }

    fn cmd_agent(&mut self, arg: &str) {
        if arg.is_empty() {
            self.push_sys("/agent <name>: <goal>   or   /agent research the repo");
            return;
        }
        let action = match arg.split_once(':') {
            Some((n, g)) => Action::Agent {
                name: slug(n),
                goal: g.trim().to_string(),
            },
            None => Action::Agent {
                name: "worker".into(),
                goal: arg.to_string(),
            },
        };
        let (name, goal) = match &action {
            Action::Agent { name, goal } => (name.clone(), goal.clone()),
            _ => unreachable!(),
        };
        self.messages.push(Msg {
            kind: MsgKind::Agent,
            title: format!("spawn {name}"),
            body: goal.clone(),
        });
        // Wrap the spawn as a one-action burst via a synthetic user goal.
        self.start_job(
            format!(
                "Spawn one subagent only.\nBURST:\n  agent {name}: {goal}\nASSERT:\n  exit 0\n"
            ),
            false,
            None,
        );
    }

    fn cmd_providers(&mut self, arg: &str) {
        if !arg.is_empty() {
            if let Some(p) = providers::find(arg) {
                self.provider = p.id.into();
                self.model = p.default_model.into();
                let mut cfg = IceConfig::load();
                cfg.provider = p.id.into();
                cfg.model = p.default_model.into();
                cfg.base_url = Some(p.base_url.into());
                cfg.apply_env();
                let _ = cfg.save();
                self.push_sys(&format!(
                    "provider {} · default model {} · key env {:?}",
                    p.id, p.default_model, p.key_envs
                ));
                return;
            }
        }
        let ready = providers::detect_ready();
        let mut body = String::from("providers:\n");
        for p in PROVIDERS {
            let mark = if ready.iter().any(|(r, _)| r.id == p.id) {
                "●"
            } else {
                "○"
            };
            let cur = if p.id == self.provider { " ←" } else { "" };
            body.push_str(&format!(
                "  {mark} {:<12}  {}  {}\n",
                p.id, p.default_model, cur
            ));
        }
        self.push_sys(&body);
    }

    fn cmd_models(&mut self) {
        let p = providers::find(&self.provider);
        match p {
            Some(p) => match providers::list_models(p) {
                Ok(list) => {
                    self.models = list.clone();
                    let preview: Vec<_> = list.into_iter().take(40).collect();
                    self.push_sys(&format!(
                        "{} models on {}:\n  {}",
                        self.models.len(),
                        p.id,
                        preview.join("\n  ")
                    ));
                }
                Err(e) => self.push_sys(&format!("model discovery failed: {e}")),
            },
            None => self.push_sys("unknown provider · /providers"),
        }
    }

    fn cmd_clis(&mut self) {
        let mut body = String::from("installed CLIs ICE can bind:\n");
        for c in providers::detect_clis() {
            body.push_str(&format!(
                "  {}  {:<10}  {}\n",
                if c.present { "●" } else { "○" },
                c.id,
                c.bin
            ));
        }
        self.push_sys(&body);
    }

    fn cmd_agents(&mut self) {
        let dir = self.root.join(".ice/agents");
        let mut body = String::from("subagents on disk:\n");
        match std::fs::read_dir(&dir) {
            Ok(rd) => {
                let mut any = false;
                for e in rd.flatten() {
                    any = true;
                    body.push_str(&format!("  {}\n", e.path().display()));
                }
                if !any {
                    body.push_str("  (none yet)\n");
                }
            }
            Err(_) => body.push_str("  (no .ice/agents yet)\n"),
        }
        self.push_sys(&body);
    }

    fn push_sys(&mut self, s: &str) {
        self.messages.push(Msg {
            kind: MsgKind::System,
            title: "ice".into(),
            body: s.into(),
        });
        if self.screen == Screen::Welcome {
            self.screen = Screen::Session;
        }
    }

    fn start_chat(&mut self, text: String) {
        if self.screen == Screen::Welcome {
            self.screen = Screen::Session;
        }
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.running = true;
        self.turns += 1;
        self.status = "replying".into();
        let job = harness::Job {
            root: self.root.clone(),
            goal: text,
            prior_delta: None,
            max_turns: 1,
            demo: self.demo,
            loop_mode: false,
            chat_mode: true,
            durable: None,
            tx,
        };
        thread::spawn(move || harness::run(job));
    }

    fn start_job(&mut self, goal: String, loop_mode: bool, durable: Option<DurableGoal>) {
        if self.screen == Screen::Welcome {
            self.screen = Screen::Session;
        }
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.running = true;
        self.turns += 1;
        self.status = if loop_mode {
            "goal loop · compiling".into()
        } else {
            "intent · compiling burst".into()
        };
        let job = harness::Job {
            root: self.root.clone(),
            goal,
            prior_delta: self.last_delta.clone(),
            max_turns: if loop_mode { 3 } else { 4 },
            demo: self.demo,
            loop_mode,
            chat_mode: false,
            durable,
            tx,
        };
        thread::spawn(move || harness::run(job));
    }

    pub fn pump(&mut self) {
        let Some(rx) = self.rx.take() else { return };
        while let Ok(ev) = rx.try_recv() {
            match ev {
                Event::Tokens { input, output } => {
                    self.context_used = self.context_used.saturating_add(input + output);
                    self.context_window = theme::context_window_for(&self.model);
                }
                Event::TodoChanged => {
                    self.todos = todos::load(&self.root);
                }
                Event::Thinking => {
                    self.messages.push(Msg {
                        kind: MsgKind::Thinking,
                        title: "intent".into(),
                        body: "compiling a burst…".into(),
                    });
                    self.status = "thinking".into();
                }
                Event::ThinkingText(t) => {
                    if self.show_thinking {
                        self.messages.push(Msg {
                            kind: MsgKind::Thinking,
                            title: "think".into(),
                            body: t,
                        });
                    }
                    self.status = "thinking".into();
                }
                Event::BurstSource(src) => {
                    self.bursts += 1;
                    self.messages.push(Msg {
                        kind: MsgKind::Burst,
                        title: format!("burst {}", self.bursts),
                        body: src,
                    });
                    self.status = "compile · execute".into();
                }
                Event::Step(step) => {
                    if step.ok {
                        self.steps_ok += 1;
                    } else {
                        self.steps_fail += 1;
                    }
                    self.messages.push(Msg {
                        kind: MsgKind::Step { ok: step.ok },
                        title: step.title,
                        body: if step.output.is_empty() {
                            format!("exit {} · {}ms", step.exit, step.ms)
                        } else {
                            format!("exit {} · {}ms\n{}", step.exit, step.ms, step.output)
                        },
                    });
                    self.todos = todos::load(&self.root);
                }
                Event::Asserts(list) => {
                    for (label, ok, detail) in list {
                        self.messages.push(Msg {
                            kind: MsgKind::Assert { ok },
                            title: format!("assert {label}"),
                            body: detail,
                        });
                    }
                    self.status = "verify".into();
                }
                Event::Delta(d) => {
                    self.last_delta = Some(d.summary.clone());
                    self.status = if d.ok {
                        "delta · closed".into()
                    } else {
                        "delta · retry".into()
                    };
                }
                Event::Assistant(t) => {
                    self.messages.push(Msg {
                        kind: MsgKind::Assistant,
                        title: "ice".into(),
                        body: t,
                    });
                }
                Event::Loop { pass, max, note } => {
                    self.loop_pass = pass;
                    self.messages.push(Msg {
                        kind: MsgKind::Loop,
                        title: format!("loop {pass}/{max}"),
                        body: note,
                    });
                    self.status = format!("loop {pass}/{max}");
                }
                Event::AgentStart { name, goal } => {
                    self.agents_alive += 1;
                    self.messages.push(Msg {
                        kind: MsgKind::Agent,
                        title: format!("agent {name}"),
                        body: format!("started · {goal}"),
                    });
                }
                Event::AgentDone { name, ok, summary } => {
                    self.agents_alive = self.agents_alive.saturating_sub(1);
                    self.agents_done += 1;
                    self.messages.push(Msg {
                        kind: MsgKind::Agent,
                        title: format!("agent {name} {}", if ok { "done" } else { "fail" }),
                        body: summary,
                    });
                }
                Event::Goal { text, done } => {
                    self.goal_text = Some(text.clone());
                    self.messages.push(Msg {
                        kind: MsgKind::Goal,
                        title: if done { "goal closed" } else { "goal open" }.into(),
                        body: text,
                    });
                }
                Event::Done { turns, ok } => {
                    self.running = false;
                    self.status = if ok {
                        format!("done · {turns} burst(s)")
                    } else {
                        format!("stopped · {turns} burst(s)")
                    };
                }
                Event::Error(e) => {
                    self.running = false;
                    self.messages.push(Msg {
                        kind: MsgKind::Error,
                        title: "error".into(),
                        body: e,
                    });
                    self.status = "error".into();
                }
            }
        }
        if self.running {
            self.rx = Some(rx);
        }
    }
}

/// Decide whether a message is casual chat (fast plain reply) vs an agentic
/// task that should compile a burst and touch the workspace. Defaults to
/// agentic when unsure, so build requests never lose their file writes.
fn is_conversational(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    if t.is_empty() {
        return true;
    }
    // Pure greetings / thanks (English + Arabic), with or without punctuation.
    const GREET: &[&str] = &[
        "hi", "hey", "hello", "yo", "sup", "hola", "howdy", "thanks", "thank you", "thx", "ty",
        "ok", "okay", "cool", "nice", "who are you", "what can you do", "help",
        "سلام", "السلام عليكم", "مرحبا", "اهلا", "أهلا", "هاي", "ازيك", "عامل ايه", "شكرا",
        "مين انت", "انت مين", "تعمل ايه",
    ];
    let stripped: String = t
        .trim_end_matches(|c: char| "!.?،؟ ".contains(c))
        .to_string();
    if GREET.iter().any(|g| stripped == *g) {
        return true;
    }
    // Action / build signals → agentic (English + Arabic).
    const ACTION: &[&str] = &[
        "build", "create", "make", "write", "add", "fix", "implement", "refactor", "generate",
        "install", "deploy", "edit", "update", "remove", "delete", "rename", "run", "exec",
        "scaffold", "setup", "set up", "website", "site", "webpage", "web page", "app", "page",
        "script", "file", "component", "function", "test", "server", "api", "readme",
        "اعمل", "إعمل", "اعملي", "انشئ", "أنشئ", "صمم", "اكتب", "أكتب", "ابني", "ابن", "عدل",
        "أضف", "اضف", "صلح", "شغل", "نفذ", "موقع", "تطبيق", "صفحة", "ملف", "سكريبت", "برنامج",
    ];
    if ACTION.iter().any(|a| t.contains(a)) {
        return false;
    }
    // Short question with no action signal → treat as chat.
    let words = t.split_whitespace().count();
    if words <= 12 && (t.ends_with('?') || t.ends_with('؟')) {
        return true;
    }
    // Very short, no action signal → chat.
    words <= 4
}

fn session_id() -> String {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{n:x}").chars().rev().take(8).collect()
}

fn slug(s: &str) -> String {
    let s: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "worker".into()
    } else {
        s.chars().take(24).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::is_conversational;

    #[test]
    fn greetings_and_smalltalk_are_chat() {
        for m in ["hi", "hello", "Hey!", "thanks", "شكرا", "سلام", "who are you?"] {
            assert!(is_conversational(m), "{m:?} should be chat");
        }
    }

    #[test]
    fn build_requests_are_agentic() {
        for m in [
            "make me a website",
            "اعمل ليا موقع",
            "create an index.html landing page",
            "fix the bug in app.rs",
            "اكتب سكريبت بايثون",
        ] {
            assert!(!is_conversational(m), "{m:?} should be agentic");
        }
    }
}
