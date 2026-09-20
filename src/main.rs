mod agents;
mod app;
mod delta;
mod exec;
mod goal;
mod harness;
mod init;
mod ir;
mod llm;
mod mascot;
mod mcp;
mod providers;
mod sandbox;
mod skills;
mod theme;
mod todos;
mod ui;
mod update;
mod verify;

use anyhow::Result;
use app::App;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::env;
use std::io::{self, stdout};
use std::path::PathBuf;
use std::time::Duration;

struct Cli {
    cwd: Option<PathBuf>,
    prompt: Option<String>,
    demo: bool,
    max_turns: u32,
    goal_loop: bool,
    version: bool,
    inspect: bool,
    init: bool,
    init_path: Option<PathBuf>,
    update: bool,
    update_check: bool,
}

fn parse_cli() -> Cli {
    let mut cli = Cli {
        cwd: None,
        prompt: None,
        demo: false,
        max_turns: 4,
        goal_loop: false,
        version: false,
        inspect: false,
        init: false,
        init_path: None,
        update: false,
        update_check: false,
    };
    let mut args = env::args().skip(1).peekable();
    while let Some(a) = args.next() {
        match a.as_str() {
            "-h" | "--help" | "help" => {
                print_help();
                std::process::exit(0);
            }
            "-V" | "--version" | "version" => cli.version = true,
            "update" | "upgrade" | "self-update" => {
                cli.update = true;
                if let Some(n) = args.peek() {
                    if n.as_str() == "--check" {
                        cli.update_check = true;
                        args.next();
                    }
                }
            }
            "--check" => cli.update_check = true,
            "inspect" => cli.inspect = true,
            "init" => {
                cli.init = true;
                if let Some(n) = args.peek() {
                    if !n.starts_with('-') {
                        cli.init_path = args.next().map(PathBuf::from);
                    }
                }
            }
            "--demo" => cli.demo = true,
            "--goal-loop" | "--loop" => cli.goal_loop = true,
            "--cwd" => cli.cwd = args.next().map(PathBuf::from),
            "-p" | "--prompt" => cli.prompt = args.next(),
            "--max-turns" => {
                cli.max_turns = args.next().and_then(|s| s.parse().ok()).unwrap_or(4);
            }
            other if other.starts_with("-p=") => {
                cli.prompt = Some(other[3..].to_string());
            }
            other => {
                eprintln!("unknown arg: {other}");
                print_help();
                std::process::exit(2);
            }
        }
    }
    cli
}

fn print_help() {
    println!(
        "ice {} — Intent · Compile · Execute\n\n\
         ice                     interactive TUI\n\
         ice -p <goal>           headless burst\n\
         ice -p <goal> --goal-loop --demo\n\
         ice init [DIR]          write a full .ice/ workspace\n\
         ice update [--check]    fetch & install the latest release\n\
         ice inspect | ice version\n\n\
         flags: --cwd DIR  --demo  --goal-loop  --max-turns N",
        env!("CARGO_PKG_VERSION")
    );
}

fn main() -> Result<()> {
    let cli = parse_cli();
    if cli.version {
        println!("ice {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if cli.update {
        return run_update(cli.update_check);
    }
    if cli.inspect {
        inspect();
        return Ok(());
    }
    if cli.init {
        let root = cli
            .init_path
            .or(cli.cwd)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let ice = crate::init::ensure(&root)?;
        println!("{}", crate::init::describe(&root));
        println!("wrote {}", ice.display());
        return Ok(());
    }

    let root = cli
        .cwd
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let _ = crate::init::ensure(&root);

    if let Some(prompt) = cli.prompt {
        return headless(root, prompt, cli.demo, cli.max_turns, cli.goal_loop);
    }

    tui(root, cli.demo)
}

fn run_update(check_only: bool) -> Result<()> {
    println!("ice {} — checking for updates…", update::current_version());
    let latest = update::fetch_latest()?;
    if !update::is_newer(&latest, update::current_version()) {
        println!("already up to date ({latest}).");
        return Ok(());
    }
    println!("new version available: {latest}");
    if check_only {
        println!("run `ice update` to install it.");
        return Ok(());
    }
    println!("downloading and verifying {latest}…");
    let path = update::install(&latest)?;
    println!("installed {latest} → {}", path.display());
    println!("restart ICE to run the new version.");
    Ok(())
}

fn inspect() {
    println!("ICE harness");
    println!("  version     {}", env!("CARGO_PKG_VERSION"));
    println!(
        "  cwd         {}",
        std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "?".into())
    );
    println!(
        "  model       {}",
        std::env::var("ICE_MODEL").unwrap_or_else(|_| "grok-3".into())
    );
    println!(
        "  base_url    {}",
        std::env::var("ICE_BASE_URL").unwrap_or_else(|_| "https://api.x.ai/v1".into())
    );
    let key = crate::llm::Llm::from_env().is_some();
    println!("  api_key     {}", if key { "set" } else { "missing" });
    println!("  loop        Intent → Compile → Execute → Verify → Δ");
}

fn headless(
    root: PathBuf,
    goal: String,
    demo: bool,
    max_turns: u32,
    goal_loop: bool,
) -> Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let durable = if goal_loop {
        let (text, done, max) = crate::goal::parse_goal_arg(&goal);
        Some(crate::goal::DurableGoal::new(
            text,
            done,
            max.max(max_turns),
        ))
    } else {
        None
    };
    let job = harness::Job {
        root,
        goal,
        prior_delta: None,
        max_turns,
        demo: demo || crate::llm::Llm::from_env().is_none(),
        loop_mode: goal_loop,
        chat_mode: false,
        durable,
        tx,
    };
    std::thread::spawn(move || harness::run(job));
    while let Ok(ev) = rx.recv() {
        match ev {
            harness::Event::Tokens { input, output } => {
                eprintln!("tokens in={input} out={output}");
            }
            harness::Event::TodoChanged => {}
            harness::Event::Thinking => eprintln!("intent…"),
            harness::Event::ThinkingText(t) => {
                eprintln!("-- think --\n{t}");
            }
            harness::Event::BurstSource(s) => {
                println!("-- burst --\n{s}");
            }
            harness::Event::Step(s) => {
                let mark = if s.ok { "ok" } else { "fail" };
                println!("[{mark}] {}  exit={} {}ms", s.title, s.exit, s.ms);
                if !s.output.is_empty() {
                    println!("{}", s.output);
                }
            }
            harness::Event::Asserts(a) => {
                for (l, ok, d) in a {
                    println!("assert {} {} — {}", if ok { "✓" } else { "✗" }, l, d);
                }
            }
            harness::Event::Delta(d) => println!("-- delta --\n{}", d.summary),
            harness::Event::Assistant(t) => println!("{t}"),
            harness::Event::Loop { pass, max, note } => {
                eprintln!("loop {pass}/{max} {note}");
            }
            harness::Event::AgentStart { name, goal } => {
                eprintln!("agent {name} start · {goal}");
            }
            harness::Event::AgentDone { name, ok, .. } => {
                eprintln!("agent {name} {}", if ok { "done" } else { "fail" });
            }
            harness::Event::Goal { text, done } => {
                eprintln!("goal {} · {text}", if done { "closed" } else { "open" });
            }
            harness::Event::Done { turns, ok } => {
                println!("done turns={turns} ok={ok}");
                break;
            }
            harness::Event::Error(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

fn tui(root: PathBuf, demo: bool) -> Result<()> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let mut term = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut app = App::new(root, demo);
    // Always-on background updater (opt out with ICE_NO_AUTO_UPDATE).
    if std::env::var_os("ICE_NO_AUTO_UPDATE").is_none() {
        let (utx, urx) = std::sync::mpsc::channel();
        app.update_rx = Some(urx);
        app.update_tx = Some(utx.clone());
        std::thread::spawn(move || update::auto_update_loop(utx));
    }
    let res = run_loop(&mut term, &mut app);
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    res
}

fn run_loop(term: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    let started = std::time::Instant::now();
    loop {
        app.tick = if std::env::var_os("ICE_REDUCED_MOTION").is_some() {
            0
        } else {
            started.elapsed().as_millis() as u64 / 80
        };
        app.pump();
        app.pump_updates();
        term.draw(|f| ui::draw(f, app))?;
        if app.should_quit {
            break;
        }
        if !event::poll(Duration::from_millis(80))? {
            continue;
        }
        match event::read()? {
            Event::Key(k) if k.kind == KeyEventKind::Press => match k.code {
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.should_quit = true;
                }
                KeyCode::Char('l') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.help = !app.help;
                }
                KeyCode::Esc => {
                    if app.help {
                        app.help = false;
                    } else if app.screen == app::Screen::Onboard {
                        app.skip_onboard();
                    } else if app.screen == app::Screen::Session && app.input.is_empty() {
                        app.screen = app::Screen::Welcome;
                    }
                }
                _ if app.help => {}
                KeyCode::PageUp => app.scroll_up(10),
                KeyCode::PageDown => app.scroll_down(10),
                KeyCode::Home => app.cursor = 0,
                KeyCode::End => app.cursor = app.input.len(),
                KeyCode::Tab => app.on_tab(),
                KeyCode::Enter => app.submit(),
                KeyCode::Backspace => app.backspace(),
                KeyCode::Left => app.move_left(),
                KeyCode::Right => app.move_right(),
                KeyCode::Up => {
                    if app.screen == app::Screen::Onboard {
                        if app.onboard_step == 0 && app.onboard_idx > 0 {
                            app.onboard_idx -= 1;
                        } else if app.onboard_step == 2 && app.model_idx > 0 {
                            app.model_idx -= 1;
                        }
                    } else {
                        app.scroll_up(1);
                    }
                }
                KeyCode::Down => {
                    if app.screen == app::Screen::Onboard {
                        if app.onboard_step == 0 {
                            app.onboard_idx =
                                (app.onboard_idx + 1).min(crate::providers::PROVIDERS.len() - 1);
                        } else if app.onboard_step == 2 && !app.models.is_empty() {
                            app.model_idx = (app.model_idx + 1).min(app.models.len() - 1);
                        }
                    } else {
                        app.scroll_down(1);
                    }
                }
                KeyCode::Char(c) => app.insert(c),
                _ => {}
            },
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
    Ok(())
}
