mod audio;
mod benchmark;
mod capability_runtime;
mod computer_runtime;
mod config;
mod execute;
mod lsp;
mod mcp;
mod mcp_runtime;
mod pals;
mod sandbox;
mod tui;
use clap::{Parser, Subcommand};
use namzu_core::{Cancellation, Completion, Limits, ScriptedProvider, ToolCall, Turn};
use namzu_tools::FileTools;
use serde_json::json;
use std::{path::PathBuf, process::ExitCode, time::Instant};

#[derive(Parser)]
#[command(name = "namzu-rs", version, about = "Native Rust agent reconstruction")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Inspect selected capability/tool permissions offline or manage a retained exec computer.
    Capabilities(Box<capability_runtime::CapabilityArgs>),
    /// Explicit owned Docker sandbox policies and bounded offline operations.
    Sandbox(Box<sandbox::SandboxArgs>),
    /// Explicit trusted LSP server and hash-bound document edits.
    Lsp(Box<lsp::LspArgs>),
    /// Explicit bounded WAV inspection, speech-to-text and text-to-speech.
    Audio(Box<audio::AudioArgs>),
    /// Offline filesystem/tool replay. No model calls or credentials.
    Demo {
        #[arg(long)]
        json: bool,
    },
    /// Run a prompt using a selected provider and private session storage.
    Exec(Box<execute::ExecArgs>),
    /// Interactive terminal with live Codex text and agent/tool events.
    Tui(Box<tui::TuiArgs>),
    /// Persistent Pals, bounded teams, mailbox workers and owned computers.
    Pal(Box<pals::PalArgs>),
    /// Explicit MCP profiles, project trust and connection checks.
    Mcp(Box<mcp::Args>),
    /// List or inspect private sessions. Output may contain private task content.
    Session {
        #[arg(long)]
        state_dir: Option<PathBuf>,
        #[command(subcommand)]
        action: SessionAction,
    },
    /// Create or show private default configuration (contains selectors, not keys).
    Config {
        #[arg(long)]
        state_dir: Option<PathBuf>,
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Native Codex subscription device login and private credential management.
    Auth {
        #[arg(long)]
        state_dir: Option<PathBuf>,
        #[command(subcommand)]
        action: AuthAction,
    },
    /// Discover the signed-in Codex subscription's actual model catalogue.
    Models {
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
    /// Deterministic benchmark harness.
    Bench {
        #[command(subcommand)]
        action: benchmark::BenchAction,
    },
    #[command(hide = true)]
    FixtureTask {
        task: String,
        #[arg(long)]
        cwd: PathBuf,
        #[arg(long)]
        broken: bool,
    },
}

#[derive(Subcommand)]
enum SessionAction {
    Computer {
        id: String,
        #[command(subcommand)]
        action: SessionComputerAction,
    },
    List,
    Show {
        id: String,
    },
}
#[derive(Subcommand)]
enum SessionComputerAction {
    Show,
    Read { path: String },
    Cleanup,
}
#[derive(Subcommand)]
enum ConfigAction {
    Init,
    Show,
}

#[derive(Subcommand)]
enum AuthAction {
    Login {
        #[arg(long,default_value_t=900,value_parser=clap::value_parser!(u64).range(1..=900))]
        timeout: u64,
    },
    Status,
    Logout,
    /// Copy access/identity tokens, never the refresh grant, to a NEW Jcode profile.
    LinkJcode {
        #[arg(long)]
        directory: PathBuf,
    },
}
fn auth_command(
    state_dir: Option<PathBuf>,
    action: AuthAction,
    cancellation: Cancellation,
) -> Result<(), namzu_core::AgentError> {
    let path = state_dir.unwrap_or(namzu_session::default_state_dir()?);
    match action {
        AuthAction::Login { timeout } => {
            let context = namzu_core::TurnContext::new(
                std::time::Duration::from_secs(timeout),
                cancellation,
            )?;
            namzu_provider::codex::auth::login_device(&path, &context, |url, code| {
                eprintln!("Open {url} and enter code: {code}")
            })?;
            println!("{}", json!({"authenticated":true}));
        }
        AuthAction::Status => println!("{}", namzu_provider::codex::auth::status(&path)?),
        AuthAction::LinkJcode { directory } => {
            namzu_provider::codex::auth::link_jcode(
                &path,
                &directory,
                &namzu_core::TurnContext::new(std::time::Duration::from_secs(30), cancellation)?,
            )?;
            println!("{}", json!({"linked":true,"refresh_grant_copied":false}));
        }
        AuthAction::Logout => {
            namzu_provider::codex::auth::logout(&path)?;
            println!("{}", json!({"authenticated":false}));
        }
    }
    Ok(())
}

fn state_command(
    state_dir: Option<PathBuf>,
    session: Option<SessionAction>,
    config: Option<ConfigAction>,
) -> Result<(), namzu_core::AgentError> {
    use namzu_session::{PrivateStore, Session, default_state_dir};
    let state = state_dir.unwrap_or(default_state_dir()?);
    let store = PrivateStore::open(&state)?;
    if let Some(action) = session {
        match action {
            SessionAction::Computer { id, action } => {
                let manager = computer_runtime::manager(&state, &id)?;
                let ctx = namzu_core::TurnContext::new(
                    std::time::Duration::from_secs(30),
                    Cancellation::default(),
                )?;
                let value = match action {
                    SessionComputerAction::Show => json!(manager.snapshot()?),
                    SessionComputerAction::Cleanup => {
                        manager.recover_cleanup(&ctx)?;
                        json!({"cleaned":true})
                    }
                    SessionComputerAction::Read { path } => {
                        let mut computer = manager.resume(&ctx)?;
                        computer.preserve_workspace_on_drop();
                        if computer.snapshot().phase == namzu_computer::Phase::Stopped {
                            computer.start(&ctx)?;
                        }
                        let bytes = computer.download(&path, &ctx)?;
                        computer.detach(&ctx)?;
                        use base64::Engine as _;
                        json!({"base64":base64::engine::general_purpose::STANDARD.encode(bytes)})
                    }
                };
                println!("{value}");
            }
            SessionAction::List => {
                let mut values = vec![];
                for name in store.names()? {
                    if name.starts_with("s-") && name.ends_with(".json") {
                        let id = name.trim_end_matches(".json");
                        let item = Session::load(&store, id)?;
                        values.push(json!({"id":id,"status":item.status,"updated_ms":item.updated_ms,"pending_call":item.pending_call}));
                    }
                }
                println!("{}", json!({"sessions":values}));
            }
            SessionAction::Show { id } => println!(
                "{}",
                serde_json::to_string(&Session::load(&store, &id)?)
                    .map_err(|_| namzu_core::AgentError::State("cannot encode session"))?
            ),
        }
    }
    if let Some(action) = config {
        match action {
            ConfigAction::Init => {
                let _lock = store.lock("config")?;
                if store.read::<serde_json::Value>("config.json")?.is_some() {
                    return Err(namzu_core::AgentError::State("config already exists"));
                }
                store.write("config.json", &config::Config::default())?;
            }
            ConfigAction::Show => println!(
                "{}",
                serde_json::to_string(&config::Config::load(&store, None)?)
                    .map_err(|_| namzu_core::AgentError::State("cannot encode config"))?
            ),
        }
    }
    Ok(())
}

fn offline_demo(cancellation: Cancellation) -> Result<Turn, String> {
    offline_demo_observed(cancellation, &mut namzu_core::NoopEvents)
}
fn offline_demo_observed(
    cancellation: Cancellation,
    events: &mut dyn namzu_core::EventSink,
) -> Result<Turn, String> {
    let root = tempfile::tempdir().map_err(|_| "cannot create demo workspace")?;
    let mut tools = FileTools::new(root.path(), true).map_err(|_| "cannot open demo workspace")?;
    let mut provider = ScriptedProvider(
        vec![
            Completion {
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "demo-write".into(),
                    name: "write_file".into(),
                    arguments: json!({"path":"answer.txt", "content":"12\n"}),
                }],
            },
            Completion {
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "demo-read".into(),
                    name: "read_file".into(),
                    arguments: json!({"path":"answer.txt"}),
                }],
            },
            Completion {
                text: "12".into(),
                tool_calls: vec![],
            },
        ]
        .into(),
    );
    let result = namzu_core::run_history(
        &mut provider,
        &mut tools,
        vec![namzu_core::Message::User {
            content: "Write and read the known answer 12.".into(),
        }],
        Limits::default(),
        cancellation,
        &mut DemoObserver(events),
    )
    .map_err(|e| e.to_string())?;
    let content = std::fs::read_to_string(root.path().join("answer.txt"))
        .map_err(|_| "cannot verify demo file")?;
    let read_verified = result.messages.iter().any(|message| matches!(message, namzu_core::Message::Tool {tool_call_id, content, is_error:false} if tool_call_id == "demo-read" && content == "12\n"));
    if content != "12\n" || !read_verified || result.stats.tool_calls != 2 {
        return Err("demo did not satisfy filesystem oracle".into());
    }
    Ok(result)
}

struct DemoObserver<'a>(&'a mut dyn namzu_core::EventSink);
impl namzu_core::Observer for DemoObserver<'_> {
    fn event(&mut self, event: namzu_core::Event) -> Result<(), namzu_core::AgentError> {
        self.0.emit(event)
    }
    fn checkpoint(
        &mut self,
        _: &[namzu_core::Message],
        _: &namzu_core::Stats,
        _: Option<&ToolCall>,
    ) -> Result<(), namzu_core::AgentError> {
        Ok(())
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let cancellation = Cancellation::default();
    let signal = cancellation.clone();
    if ctrlc::set_handler(move || signal.cancel()).is_err() {
        eprintln!("cannot install interrupt handler");
        return ExitCode::FAILURE;
    }
    let started = Instant::now();
    let (json_output, offline, session_id, result) = match cli.command {
        Command::Capabilities(args) => {
            return match capability_runtime::inspect(*args, cancellation) {
                Ok(value) => {
                    println!(
                        "{}",
                        json!({"format_version":1,"status":"ok","output":value})
                    );
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    println!(
                        "{}",
                        json!({"format_version":1,"status":"error","error":error.to_string(),"provider_error":error.provider_failure()})
                    );
                    ExitCode::FAILURE
                }
            };
        }
        Command::Sandbox(args) => {
            return match sandbox::run(*args, cancellation) {
                Ok(value) => {
                    let failed = value.get("error").is_some_and(|v| !v.is_null())
                        || value.get("result").is_some_and(|r| {
                            r.get("status").is_some_and(|s| s != "exited")
                                || r.get("exit_code").is_some_and(|c| c != 0)
                        });
                    println!(
                        "{}",
                        json!({"format_version":1,"status":if failed {"error"}else{"ok"},"output":value})
                    );
                    if failed {
                        ExitCode::FAILURE
                    } else {
                        ExitCode::SUCCESS
                    }
                }
                Err(error) => {
                    println!(
                        "{}",
                        json!({"format_version":1,"status":"error","error":error.to_string(),"provider_error":error.provider_failure()})
                    );
                    ExitCode::FAILURE
                }
            };
        }
        Command::Lsp(args) => {
            return match lsp::run(*args, cancellation) {
                Ok(value) => {
                    println!(
                        "{}",
                        json!({"format_version":1,"status":"ok","output":value})
                    );
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    println!(
                        "{}",
                        json!({"format_version":1,"status":"error","error":error.to_string(),"provider_error":error.provider_failure()})
                    );
                    ExitCode::FAILURE
                }
            };
        }
        Command::Audio(args) => {
            return match audio::run(*args, cancellation) {
                Ok(value) => {
                    println!(
                        "{}",
                        json!({"format_version":1,"status":"ok","output":value})
                    );
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    println!(
                        "{}",
                        json!({"format_version":1,"status":"error","error":error.to_string(),"provider_error":error.provider_failure()})
                    );
                    ExitCode::FAILURE
                }
            };
        }
        Command::Mcp(args) => {
            return match mcp::run(*args, cancellation) {
                Ok(value) => {
                    println!(
                        "{}",
                        json!({"format_version":1,"status":"ok","output":value})
                    );
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    println!(
                        "{}",
                        json!({"format_version":1,"status":"error","error":error.to_string()})
                    );
                    ExitCode::FAILURE
                }
            };
        }
        Command::Pal(args) => {
            return match pals::run(*args, cancellation) {
                Ok(true) => ExitCode::SUCCESS,
                Ok(false) => ExitCode::FAILURE,
                Err(error) => {
                    println!(
                        "{}",
                        json!({"format_version":1,"status":"error","error":error.to_string(),"provider_error":error.provider_failure()})
                    );
                    ExitCode::FAILURE
                }
            };
        }
        Command::Tui(args) => {
            return match tui::run(*args, cancellation) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::FAILURE
                }
            };
        }
        Command::Bench { action } => {
            return match benchmark::run(action, cancellation) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::FAILURE
                }
            };
        }
        Command::FixtureTask { task, cwd, broken } => {
            return match benchmark::fixture(&task, &cwd, broken, cancellation) {
                Ok(value) => {
                    println!("{value}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::FAILURE
                }
            };
        }
        Command::Auth { state_dir, action } => {
            return match auth_command(state_dir, action, cancellation) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::FAILURE
                }
            };
        }
        Command::Models { state_dir } => {
            let result = (|| {
                let driver = namzu_provider::codex::CodexProvider::new(
                    state_dir.unwrap_or(namzu_session::default_state_dir()?),
                    "catalogue".into(),
                    None,
                )?;
                let models = driver.models(&namzu_core::TurnContext::new(
                    std::time::Duration::from_secs(30),
                    cancellation,
                )?)?;
                println!("{models}");
                Ok::<_, namzu_core::AgentError>(())
            })();
            return match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::FAILURE
                }
            };
        }
        Command::Demo { json } => (
            json,
            true,
            None,
            offline_demo(cancellation).map_err(|e| (e, None)),
        ),
        Command::Exec(args) => {
            let json = args.json;
            let result = execute::execute(*args, cancellation);
            (
                json,
                false,
                result.session_id,
                result
                    .result
                    .map_err(|e| (e.to_string(), e.provider_failure().cloned())),
            )
        }
        Command::Session { state_dir, action } => {
            return match state_command(state_dir, Some(action), None) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::FAILURE
                }
            };
        }
        Command::Config { state_dir, action } => {
            return match state_command(state_dir, None, Some(action)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::FAILURE
                }
            };
        }
    };
    let elapsed_ms = started.elapsed().as_millis();
    match result {
        Ok(turn) => {
            if json_output {
                println!(
                    "{}",
                    json!({"format_version":1,"status":"ok","output":turn.output,"stats":turn.stats,"elapsed_ms":elapsed_ms,"offline":offline,"session_id":session_id})
                );
            } else {
                println!("{}", turn.output);
            }
            ExitCode::SUCCESS
        }
        Err((error, provider_error)) => {
            if json_output {
                println!(
                    "{}",
                    json!({"format_version":1,"status":"error","error":error,"provider_error":provider_error,"elapsed_ms":elapsed_ms,"offline":offline,"session_id":session_id})
                );
            } else {
                eprintln!("{error}");
            }
            ExitCode::FAILURE
        }
    }
}
