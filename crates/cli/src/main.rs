use clap::{Parser, Subcommand};
use namzu_core::{Cancellation, Completion, Limits, ScriptedProvider, ToolCall, Turn, run_turn};
use namzu_provider::OpenAiCompatible;
use namzu_tools::FileTools;
use serde_json::json;
use std::{
    path::PathBuf,
    process::ExitCode,
    time::{Duration, Instant},
};

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
    /// Run a prompt using an explicitly configured OpenAI-compatible API.
    Exec {
        prompt: String,
        #[arg(long, default_value = ".")]
        cwd: PathBuf,
        #[arg(long)]
        model: String,
        #[arg(long, default_value = "https://api.openai.com/v1")]
        base_url: String,
        #[arg(long, default_value = "OPENAI_API_KEY")]
        api_key_env: String,
        #[arg(long, default_value_t=300, value_parser=clap::value_parser!(u64).range(1..=86400))]
        timeout: u64,
        #[arg(long, default_value_t = 64)]
        max_steps: usize,
        #[arg(long, default_value_t = 128)]
        max_tool_calls: usize,
        /// Grant writes within --cwd. Without this flag only read/list tools exist.
        #[arg(long)]
        allow_write: bool,
        #[arg(long)]
        json: bool,
    },
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
    let (json_output, offline, result) = match cli.command {
        Command::Demo { json } => (json, true, offline_demo(cancellation)),
        Command::Exec {
            prompt,
            cwd,
            model,
            base_url,
            api_key_env,
            timeout,
            max_steps,
            max_tool_calls,
            allow_write,
            json,
        } => {
            let result = (|| {
                if max_steps == 0 {
                    return Err("max-steps must be positive".to_owned());
                }
                let key = std::env::var(&api_key_env)
                    .map_err(|_| "API key environment variable is missing or invalid".to_owned())?;
                let mut provider =
                    OpenAiCompatible::new(&base_url, model, &key).map_err(|e| e.to_string())?;
                let mut tools = FileTools::new(cwd, allow_write)
                    .map_err(|_| "cannot open workspace".to_owned())?;
                run_turn(
                    &mut provider,
                    &mut tools,
                    &prompt,
                    Limits {
                        timeout: Duration::from_secs(timeout),
                        max_steps,
                        max_tool_calls,
                        ..Limits::default()
                    },
                    cancellation,
                )
                .map_err(|e| e.to_string())
            })();
            (json, false, result)
        }
    };
    let elapsed_ms = started.elapsed().as_millis();
    match result {
        Ok(turn) => {
            if json_output {
                println!(
                    "{}",
                    json!({"format_version":1,"status":"ok","output":turn.output,"stats":turn.stats,"elapsed_ms":elapsed_ms,"offline":offline})
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
                    json!({"format_version":1,"status":"error","error":error,"elapsed_ms":elapsed_ms,"offline":offline})
                );
            } else {
                eprintln!("{error}");
            }
            ExitCode::FAILURE
        }
    }
}
