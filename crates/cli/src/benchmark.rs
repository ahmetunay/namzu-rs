use clap::{Subcommand, ValueEnum};
use namzu_bench::{Agent, Suite};
use namzu_core::{
    AgentError, Cancellation, Completion, Limits, ScriptedProvider, ToolCall, run_turn,
};
use serde_json::json;
use std::{
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, ValueEnum)]
pub enum TaskSet {
    Algorithms,
    Eval,
}
impl TaskSet {
    fn ids(self) -> &'static [&'static str] {
        match self {
            Self::Algorithms => &namzu_bench::BUILTIN_TASKS[..3],
            Self::Eval => namzu_bench::BUILTIN_TASKS,
        }
    }
}

#[derive(Subcommand)]
pub enum BenchAction {
    /// Validate suite, commands and oracle configuration without inference.
    Validate { suite: PathBuf },
    /// Run sequential agents with independent oracles. Live mode consumes quota.
    Run {
        suite: PathBuf,
        #[arg(long, default_value = "artifacts/bench")]
        out: PathBuf,
    },
    /// Offline gates: known fixture writer vs a deliberately incorrect no-op.
    Demo {
        #[arg(long, value_enum, default_value = "algorithms")]
        task_set: TaskSet,
        #[arg(long, default_value = "artifacts/bench")]
        out: PathBuf,
    },
    /// Write a live Namzu/Jcode suite; no model calls.
    Init {
        #[arg(long, value_enum, default_value = "algorithms")]
        task_set: TaskSet,
        #[arg(long)]
        model: String,
        #[arg(long, default_value = "jcode")]
        jcode: PathBuf,
        #[arg(long)]
        file: PathBuf,
    },
}
pub fn fixture(
    task: &str,
    cwd: &Path,
    broken: bool,
    cancellation: Cancellation,
) -> Result<serde_json::Value, AgentError> {
    let solution = namzu_bench::fixture_solution(task)?;
    let mut script = vec![];
    if !broken {
        script.push(Completion {
            text: "".into(),
            tool_calls: vec![ToolCall {
                id: "fixture-write".into(),
                name: "write_file".into(),
                arguments: json!({"path":"solution.py","content":solution}),
            }],
        });
    }
    script.push(Completion {
        text: "done".into(),
        tool_calls: vec![],
    });
    let mut provider = ScriptedProvider(script.into());
    let mut tools = namzu_tools::FileTools::new(cwd, true)
        .map_err(|_| AgentError::State("cannot open fixture workspace"))?;
    let result = run_turn(
        &mut provider,
        &mut tools,
        "Offline benchmark fixture",
        Limits::default(),
        cancellation,
    )?;
    Ok(
        json!({"format_version":1,"status":"ok","offline":true,"output":result.output,"stats":result.stats}),
    )
}

pub fn run(action: BenchAction, cancellation: Cancellation) -> Result<(), AgentError> {
    match action {
        BenchAction::Validate { suite } => {
            let suite = Suite::load(&suite)?;
            println!(
                "{}",
                json!({"valid":true,"mode":suite.mode,"model":suite.model,"agents":suite.agents.len(),"tasks":suite.tasks.len(),"timeout_seconds":suite.timeout_seconds})
            );
        }
        BenchAction::Run { suite, out } => {
            let suite = Suite::load(&suite)?;
            let report = namzu_bench::run(&suite, &out, cancellation, |row| {
                eprintln!(
                    "{}",
                    json!({"agent":row["agent"],"task":row["task"],"status":row["status"],"elapsed_ms":row["elapsed_ms"]})
                )
            })?;
            println!("{report}");
        }
        BenchAction::Demo { out, task_set } => {
            let program = std::env::current_exe()
                .map_err(|_| AgentError::Configuration("cannot resolve current binary"))?;
            let args = vec![
                "fixture-task".into(),
                "{task}".into(),
                "--cwd".into(),
                "{workspace}".into(),
            ];
            let mut bad = args.clone();
            bad.push("--broken".into());
            let suite = Suite {
                version: 1,
                mode: "fixture".into(),
                model: "scripted-fixture".into(),
                timeout_seconds: 300,
                repeats: 1,
                agents: vec![
                    Agent {
                        name: "fixture-correct".into(),
                        program: program.clone(),
                        args,
                        permission_profile: "fixture-file-write".into(),
                        inherit_env: vec![],
                    },
                    Agent {
                        name: "fixture-noop".into(),
                        program,
                        args: bad,
                        permission_profile: "fixture-file-write".into(),
                        inherit_env: vec![],
                    },
                ],
                tasks: task_set
                    .ids()
                    .iter()
                    .copied()
                    .map(namzu_bench::builtin_task)
                    .collect::<Result<_, _>>()?,
            };
            let report = namzu_bench::run(&suite, &out, cancellation, |_| {})?;
            println!("{report}");
        }
        BenchAction::Init {
            model,
            jcode,
            file,
            task_set,
        } => {
            if file.exists() {
                return Err(AgentError::Configuration("suite file already exists"));
            }
            let namzu = std::env::current_exe()
                .map_err(|_| AgentError::Configuration("cannot resolve Namzu binary"))?;
            let jcode = namzu_bench::executable(&jcode)?;
            let suite = Suite {
                version: 1,
                mode: "live".into(),
                model,
                timeout_seconds: 300,
                repeats: 1,
                agents: vec![
                    Agent {
                        name: "namzu-rs".into(),
                        program: namzu,
                        args: [
                            "exec",
                            "{prompt}",
                            "--provider",
                            "codex",
                            "--model",
                            "{model}",
                            "--cwd",
                            "{workspace}",
                            "--timeout",
                            "300",
                            "--allow-write",
                            "--allow-shell",
                            "--json",
                        ]
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                        permission_profile: "host-read-write-shell".into(),
                        inherit_env: vec!["NAMZU_STATE_DIR".into(), "XDG_STATE_HOME".into()],
                    },
                    Agent {
                        name: "jcode".into(),
                        program: jcode,
                        args: [
                            "run",
                            "{prompt}",
                            "--provider",
                            "openai",
                            "--model",
                            "{model}",
                            "--cwd",
                            "{workspace}",
                            "--no-update",
                            "--no-selfdev",
                            "--socket",
                            "{workspace}/jcode.sock",
                            "--tools",
                            "bash,read,write,edit,apply_patch,glob,grep",
                            "--json",
                        ]
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                        permission_profile: "host-read-write-shell".into(),
                        inherit_env: vec!["JCODE_HOME".into(), "JCODE_OPENAI_TRANSPORT".into()],
                    },
                ],
                tasks: task_set
                    .ids()
                    .iter()
                    .copied()
                    .map(namzu_bench::builtin_task)
                    .collect::<Result<_, _>>()?,
            };
            suite.validate()?;
            let bytes = serde_json::to_vec_pretty(&suite)
                .map_err(|_| AgentError::Protocol("cannot encode suite"))?;
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&file)
                .and_then(|mut file| file.write_all(&bytes))
                .map_err(|_| {
                    AgentError::State("cannot create suite without replacing an existing file")
                })?;
            println!(
                "{}",
                json!({"created":file,"mode":"live","timeout_seconds":300})
            );
        }
    }
    Ok(())
}
