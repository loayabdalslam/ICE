mod agents;
mod cli;
mod commands;
mod conv;
mod editor;
mod engine;
mod hooks;
mod http;
mod init;
mod ir;
mod mascot;
mod mcp;
mod model;
mod oauth;
mod permissions;
mod print;
mod prompt;
mod providers;
mod render;
mod repl;
mod sandbox;
mod settings;
mod skills;
mod store;
mod term;
mod theme;
mod tools;
mod update;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use permissions::Mode;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(
    name = "ice",
    version,
    about = "ICE — an agentic coding tool that lives in your terminal",
    long_about = "ICE — an agentic coding tool that lives in your terminal.\n\nStarts an interactive session by default; use -p/--print for non-interactive output.",
    after_help = "Examples:\n  ice                              start an interactive session\n  ice \"explain this project\"       start with a first prompt\n  ice -p \"list the TODOs\"          print the answer and exit\n  cat err.log | ice -p \"why?\"      pipe content in\n  ice -c                           continue the most recent conversation\n  ice -r                           pick a conversation to resume\n  ice mcp add fs -- npx -y @modelcontextprotocol/server-filesystem .",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    /// Your prompt
    prompt: Option<String>,

    /// Print the response and exit (useful for pipes)
    #[arg(short = 'p', long = "print")]
    print: bool,

    /// Output format for --print
    #[arg(long = "output-format", value_enum, default_value_t = OutputFormat::Text)]
    output_format: OutputFormat,

    /// Show tool calls and full output
    #[arg(long)]
    verbose: bool,

    /// Continue the most recent conversation in this directory
    #[arg(short = 'c', long = "continue")]
    continue_last: bool,

    /// Resume a conversation (by session ID, or pick interactively)
    #[arg(short = 'r', long = "resume", num_args = 0..=1, default_missing_value = "")]
    resume: Option<String>,

    /// Model for this session: an alias (sonnet, opus, haiku) or a full model id
    #[arg(long)]
    model: Option<String>,

    /// Permission mode for this session
    #[arg(long = "permission-mode", value_enum)]
    permission_mode: Option<PermissionModeArg>,

    /// Bypass all permission checks (only use in a sandbox without internet)
    #[arg(long = "dangerously-skip-permissions")]
    skip_permissions: bool,

    /// Tools to allow without prompting, e.g. "Bash(git log:*)" Edit
    #[arg(long = "allowedTools", alias = "allowed-tools", num_args = 1.., value_delimiter = ',')]
    allowed_tools: Vec<String>,

    /// Tools to deny, e.g. "Bash(rm:*)" WebFetch
    #[arg(long = "disallowedTools", alias = "disallowed-tools", num_args = 1.., value_delimiter = ',')]
    disallowed_tools: Vec<String>,

    /// Maximum number of agentic turns (print mode)
    #[arg(long = "max-turns")]
    max_turns: Option<u32>,

    /// Append text to the system prompt
    #[arg(long = "append-system-prompt")]
    append_system_prompt: Option<String>,

    /// Replace the system prompt (print mode)
    #[arg(long = "system-prompt")]
    system_prompt: Option<String>,

    /// Use a specific session ID (UUID)
    #[arg(long = "session-id")]
    session_id: Option<String>,

    /// Working directory (defaults to the current directory)
    #[arg(long, value_name = "DIR")]
    cwd: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
    #[value(name = "stream-json")]
    StreamJson,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum PermissionModeArg {
    Default,
    #[value(name = "acceptEdits")]
    AcceptEdits,
    Plan,
    #[value(name = "bypassPermissions")]
    BypassPermissions,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Manage settings (ice config list | get KEY | set KEY VALUE)
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },
    /// Configure and manage MCP servers
    Mcp {
        #[command(subcommand)]
        action: McpAction,
    },
    /// Check for updates and install them
    #[command(alias = "upgrade")]
    Update {
        /// Only check, don't install
        #[arg(long)]
        check: bool,
    },
    /// Sign in to a provider: browser OAuth (PKCE) when available, else API key
    Login {
        /// Provider id (anthropic, openai, gemini, openrouter, xai, groq, …)
        provider: Option<String>,
        /// Paste an API key instead of signing in with the browser
        #[arg(long)]
        api_key: bool,
    },
    /// Remove saved API keys and OAuth tokens
    Logout,
    /// Check the health of your ICE installation
    Doctor,
    /// Print environment information (alias of doctor)
    Inspect,
    /// Create .ice/ project scaffolding (settings, commands, agents)
    Init { dir: Option<PathBuf> },
    /// Generate shell completions
    Completions { shell: clap_complete::Shell },
    /// Print the version
    Version,
}

#[derive(Subcommand, Debug)]
enum ConfigAction {
    /// List all user settings
    List,
    /// Get a setting
    Get { key: String },
    /// Set a setting (value is parsed as JSON when possible)
    Set { key: String, value: String },
    /// Print the settings file paths
    Path,
}

#[derive(Subcommand, Debug)]
enum McpAction {
    /// List configured servers and their status
    List,
    /// Show one server's configuration
    Get { name: String },
    /// Add a server: ice mcp add NAME -- COMMAND [ARGS...]   or   --transport http NAME URL
    Add {
        name: String,
        /// Transport: stdio (default) or http
        #[arg(short = 't', long, default_value = "stdio")]
        transport: String,
        /// Config scope: local (.ice/mcp.json), project (.mcp.json) or user (~/.ice/mcp.json)
        #[arg(short = 's', long, default_value = "local")]
        scope: String,
        /// Environment variables (KEY=VALUE) for stdio servers
        #[arg(short = 'e', long = "env")]
        env: Vec<String>,
        /// HTTP headers (Name: value) for http servers
        #[arg(short = 'H', long = "header")]
        header: Vec<String>,
        /// Command and arguments (stdio) or URL (http)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    /// Remove a server from every config file
    Remove { name: String },
}

fn root_dir(cwd: Option<PathBuf>) -> PathBuf {
    let p = cwd.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let p = std::fs::canonicalize(&p).unwrap_or(p);
    // Strip Windows' verbatim prefix so paths look normal to users and models.
    let s = p.display().to_string();
    match s.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => p,
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let root = root_dir(cli.cwd.clone());
    let settings = settings::Settings::load(&root);
    settings.apply_env();
    settings::load_credentials();

    if let Some(cmd) = cli.command {
        return run_subcommand(cmd, &root);
    }
    if !root.is_dir() {
        eprintln!("Error: {} is not a directory", root.display());
        return ExitCode::from(2);
    }

    let mode = if cli.skip_permissions {
        Mode::BypassPermissions
    } else if let Some(m) = cli.permission_mode {
        match m {
            PermissionModeArg::Default => Mode::Default,
            PermissionModeArg::AcceptEdits => Mode::AcceptEdits,
            PermissionModeArg::Plan => Mode::Plan,
            PermissionModeArg::BypassPermissions => Mode::BypassPermissions,
        }
    } else {
        settings
            .default_mode
            .as_deref()
            .and_then(Mode::parse)
            .filter(|m| *m != Mode::BypassPermissions)
            .unwrap_or(Mode::Default)
    };
    store::cleanup(settings.cleanup_period_days);

    let print_mode = cli.print
        || !std::io::stdout().is_terminal()
        || !std::io::stdin().is_terminal() && cli.prompt.is_none();
    let opts = engine::Options {
        root: root.clone(),
        model_override: cli.model.clone(),
        mode,
        interactive: !print_mode,
        max_turns: cli.max_turns,
        allowed_tools: cli.allowed_tools.clone(),
        disallowed_tools: cli.disallowed_tools.clone(),
        append_system_prompt: cli.append_system_prompt.clone(),
        system_prompt: cli.system_prompt.clone(),
        session_id: cli.session_id.clone(),
        connect_mcp: true,
        cancel: Arc::new(AtomicBool::new(false)),
    };

    if print_mode {
        let prompt = print::read_prompt(cli.prompt.clone());
        let format = match cli.output_format {
            OutputFormat::Text => print::Format::Text,
            OutputFormat::Json => print::Format::Json,
            OutputFormat::StreamJson => print::Format::StreamJson,
        };
        let resume = match (&cli.resume, cli.continue_last) {
            (Some(id), _) if !id.is_empty() => Some(id.clone()),
            (_, true) => store::list(&root).first().map(|s| s.id.clone()),
            _ => None,
        };
        let code = print::run(opts, prompt, format, cli.verbose, resume);
        return ExitCode::from(code as u8);
    }

    let res = repl::Repl::run(repl::StartOptions {
        root,
        engine: opts,
        initial_prompt: cli.prompt,
        resume: cli.resume.clone().filter(|r| !r.is_empty()),
        continue_last: cli.continue_last,
        pick_resume: cli.resume.as_deref() == Some(""),
        bypass_available: cli.skip_permissions,
        verbose: cli.verbose,
    });
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_subcommand(cmd: Commands, root: &std::path::Path) -> ExitCode {
    match cmd {
        Commands::Version => {
            println!("{} (ICE)", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Commands::Doctor | Commands::Inspect => {
            let pool = mcp::Pool::connect(root);
            println!("{}", commands::doctor_report(root, Some(&pool)));
            ExitCode::SUCCESS
        }
        Commands::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "ice", &mut std::io::stdout());
            ExitCode::SUCCESS
        }
        Commands::Init { dir } => {
            let target = dir
                .map(|d| root_dir(Some(d)))
                .unwrap_or_else(|| root.to_path_buf());
            match init::ensure(&target) {
                Ok(created) => {
                    println!("{}", init::describe(&target, &created));
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Commands::Update { check } => match update::run_cli(check) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("Error: {e}");
                ExitCode::FAILURE
            }
        },
        Commands::Config { action } => {
            match action.unwrap_or(ConfigAction::List) {
                ConfigAction::List => println!(
                    "{}",
                    serde_json::to_string_pretty(&settings::user_json()).unwrap_or_default()
                ),
                ConfigAction::Get { key } => {
                    let v = settings::get_user_value(&key);
                    if v.is_null() {
                        eprintln!("{key} is not set");
                        return ExitCode::FAILURE;
                    }
                    println!(
                        "{}",
                        match v {
                            serde_json::Value::String(s) => s,
                            other => other.to_string(),
                        }
                    );
                }
                ConfigAction::Set { key, value } => {
                    let v =
                        serde_json::from_str(&value).unwrap_or(serde_json::Value::String(value));
                    if let Err(e) = settings::set_user_value(&key, v) {
                        eprintln!("Error: {e}");
                        return ExitCode::FAILURE;
                    }
                    println!("Set {key} in {}", settings::user_path().display());
                }
                ConfigAction::Path => {
                    println!("user:    {}", settings::user_path().display());
                    println!("project: {}", settings::project_path(root).display());
                    println!("local:   {}", settings::local_path(root).display());
                }
            }
            ExitCode::SUCCESS
        }
        Commands::Mcp { action } => mcp_command(action, root),
        Commands::Login { provider, api_key } => login_command(provider, api_key),
        Commands::Logout => {
            let n = settings::remove_credentials();
            println!(
                "Removed {n} saved credential{}.",
                if n == 1 { "" } else { "s" }
            );
            ExitCode::SUCCESS
        }
    }
}

fn prompt_line(label: &str) -> String {
    use std::io::Write;
    print!("{label}");
    let _ = std::io::stdout().flush();
    let mut s = String::new();
    let _ = std::io::stdin().read_line(&mut s);
    s.trim().to_string()
}

fn login_command(provider: Option<String>, api_key: bool) -> ExitCode {
    let id = match provider {
        Some(p) => p,
        None => {
            println!("Providers:");
            for (i, p) in providers::PROVIDERS.iter().enumerate() {
                let oauth = matches!(oauth::availability(p.id), oauth::Availability::Ready(_));
                println!(
                    "  {:>2}. {:<24} {}",
                    i + 1,
                    p.name,
                    if oauth {
                        "browser sign-in (PKCE) or API key"
                    } else {
                        "API key"
                    }
                );
            }
            let pick = prompt_line("Choose a provider [number or id]: ");
            match pick.parse::<usize>() {
                Ok(n) if n >= 1 && n <= providers::PROVIDERS.len() => {
                    providers::PROVIDERS[n - 1].id.to_string()
                }
                _ => pick,
            }
        }
    };
    let Some(p) = providers::find(&id) else {
        eprintln!("Unknown provider: {id}");
        return ExitCode::FAILURE;
    };
    let mut done = false;
    if !api_key {
        match oauth::availability(p.id) {
            oauth::Availability::Ready(cfg) => match oauth::begin(cfg) {
                Ok(pending) => {
                    println!("Opening your browser to sign in to {}…\nIf it doesn't open, visit:\n\n  {}\n", p.name, pending.url);
                    oauth::open_browser(&pending.url);
                    println!("Waiting for the browser to redirect back (Ctrl+C to cancel)…");
                    match oauth::finish(pending, &AtomicBool::new(false))
                        .and_then(|t| oauth::store(p.id, &t))
                    {
                        Ok(()) => {
                            println!("✔ Signed in to {} (OAuth 2.0 + PKCE).", p.name);
                            done = true;
                        }
                        Err(e) => {
                            eprintln!("Browser sign-in failed: {e}");
                            return ExitCode::FAILURE;
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Could not start browser sign-in: {e}");
                    return ExitCode::FAILURE;
                }
            },
            oauth::Availability::NeedsSetup(msg) => println!(
                "Browser sign-in isn't set up for {}:\n{msg}\n\nFalling back to an API key.",
                p.name
            ),
        }
    }
    if !done && p.id != "ollama" {
        let key = prompt_line(&format!(
            "Paste your {} API key ({}): ",
            p.name, p.key_envs[0]
        ));
        if key.is_empty() {
            eprintln!("No key entered.");
            return ExitCode::FAILURE;
        }
        if let Err(e) = settings::save_credential(p.key_envs[0], &key) {
            eprintln!("Error: {e}");
            return ExitCode::FAILURE;
        }
        println!(
            "✔ Saved to {} (readable only by you).",
            settings::credentials_path().display()
        );
    }
    let mut cfg = providers::IceConfig::load();
    cfg.provider = p.id.into();
    cfg.model = p.default_model.into();
    cfg.base_url = None;
    cfg.onboarded = true;
    if let Err(e) = cfg.save() {
        eprintln!("Error: {e}");
        return ExitCode::FAILURE;
    }
    println!(
        "Using {} · {} (change with /model).",
        p.name, p.default_model
    );
    ExitCode::SUCCESS
}

fn mcp_command(action: McpAction, root: &std::path::Path) -> ExitCode {
    match action {
        McpAction::List => {
            if mcp::load_specs(root).is_empty() {
                println!("No MCP servers configured. Use `ice mcp add` to add a server.");
                return ExitCode::SUCCESS;
            }
            println!("Checking MCP server health...\n");
            let pool = mcp::Pool::connect(root);
            print!("{}", mcp::describe(&pool, root));
            ExitCode::SUCCESS
        }
        McpAction::Get { name } => match mcp::load_specs(root).into_iter().find(|s| s.name == name)
        {
            Some(s) => {
                println!("{}:\n  Scope: {}", s.name, s.source);
                match s.transport {
                    mcp::Transport::Stdio { command, args, env } => {
                        println!(
                            "  Type: stdio\n  Command: {command}\n  Args: {}",
                            args.join(" ")
                        );
                        for (k, _) in env {
                            println!("  Env: {k}=…");
                        }
                    }
                    mcp::Transport::Http { url, .. } => println!("  Type: http\n  URL: {url}"),
                }
                ExitCode::SUCCESS
            }
            None => {
                eprintln!("No MCP server found with name: {name}");
                ExitCode::FAILURE
            }
        },
        McpAction::Add {
            name,
            transport,
            scope,
            env,
            header,
            command,
        } => {
            let path = match scope.as_str() {
                "project" => root.join(".mcp.json"),
                "user" => settings::user_dir().join("mcp.json"),
                _ => root.join(".ice/mcp.json"),
            };
            let t = if transport == "http" || transport == "sse" {
                let url = command.first().cloned().unwrap_or_default();
                let headers = header
                    .iter()
                    .filter_map(|h| {
                        h.split_once(':')
                            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                    })
                    .collect();
                mcp::Transport::Http { url, headers }
            } else {
                let mut it = command.into_iter();
                let cmd = it.next().unwrap_or_default();
                let envs = env
                    .iter()
                    .filter_map(|e| {
                        e.split_once('=')
                            .map(|(k, v)| (k.to_string(), v.to_string()))
                    })
                    .collect();
                mcp::Transport::Stdio {
                    command: cmd,
                    args: it.collect(),
                    env: envs,
                }
            };
            match mcp::add_server(&path, &name, t) {
                Ok(()) => {
                    println!(
                        "Added {transport} MCP server {name} to {scope} config ({})",
                        path.display()
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        McpAction::Remove { name } => match mcp::remove_server(root, &name) {
            Ok(v) if !v.is_empty() => {
                println!("Removed MCP server {name} from {}", v.join(", "));
                ExitCode::SUCCESS
            }
            Ok(_) => {
                eprintln!("No MCP server found with name: {name}");
                ExitCode::FAILURE
            }
            Err(e) => {
                eprintln!("Error: {e}");
                ExitCode::FAILURE
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parses_claude_style_flags() {
        let c = Cli::try_parse_from([
            "ice",
            "-p",
            "hello",
            "--output-format",
            "json",
            "--allowedTools",
            "Bash(git log:*)",
            "Edit",
            "--max-turns",
            "3",
        ])
        .unwrap();
        assert!(c.print);
        assert_eq!(c.prompt.as_deref(), Some("hello"));
        assert!(matches!(c.output_format, OutputFormat::Json));
        assert_eq!(c.allowed_tools, vec!["Bash(git log:*)", "Edit"]);
        assert_eq!(c.max_turns, Some(3));
        let c = Cli::try_parse_from(["ice", "-r"]).unwrap();
        assert_eq!(c.resume.as_deref(), Some(""));
        let c = Cli::try_parse_from(["ice", "mcp", "add", "fs", "--", "npx", "-y", "srv"]).unwrap();
        assert!(matches!(
            c.command,
            Some(Commands::Mcp {
                action: McpAction::Add { .. }
            })
        ));
        let c = Cli::try_parse_from(["ice", "explain this"]).unwrap();
        assert_eq!(c.prompt.as_deref(), Some("explain this"));
        Cli::command().debug_assert();
    }
}
