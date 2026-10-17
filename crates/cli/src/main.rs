mod config;
mod execute;
use clap::{Parser, Subcommand};
use namzu_core::{Cancellation, Completion, Limits, ScriptedProvider, ToolCall, Turn, run_turn};
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
    /// Offline filesystem/tool replay. No model calls or credentials.
    Demo {
        #[arg(long)]
        json: bool,
    },
    /// Run a prompt using a selected provider and private session storage.
    Exec(Box<execute::ExecArgs>),
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
}

#[derive(Subcommand)]
enum SessionAction {
    List,
    Show { id: String },
}
#[derive(Subcommand)]
enum ConfigAction {
    Init,
    Show,
}

fn state_command(
    state_dir: Option<PathBuf>,
    session: Option<SessionAction>,
    config: Option<ConfigAction>,
) -> Result<(), namzu_core::AgentError> {
    use namzu_session::{PrivateStore, Session, default_state_dir};
    let store = PrivateStore::open(state_dir.unwrap_or(default_state_dir()?))?;
    if let Some(action) = session {
        match action {
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
    let result = run_turn(
        &mut provider,
        &mut tools,
        "Write and read the known answer 12.",
        Limits::default(),
        cancellation,
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
        Command::Demo { json } => (json, true, None, offline_demo(cancellation)),
        Command::Exec(args) => {
            let json = args.json;
            let result = execute::execute(*args, cancellation);
            (
                json,
                false,
                result.session_id,
                result.result.map_err(|e| e.to_string()),
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
        Err(error) => {
            if json_output {
                println!(
                    "{}",
                    json!({"format_version":1,"status":"error","error":error,"elapsed_ms":elapsed_ms,"offline":offline,"session_id":session_id})
                );
            } else {
                eprintln!("{error}");
            }
            ExitCode::FAILURE
        }
    }
}
