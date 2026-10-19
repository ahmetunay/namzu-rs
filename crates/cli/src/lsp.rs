//! Explicit LSP operator commands; automatic agent capability integration is separate.
use clap::{Args, Subcommand};
use namzu_core::{AgentError, Cancellation, TurnContext};
use namzu_lsp::Workspace;
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
#[derive(Args)]
pub struct LspArgs {
    #[arg(long, default_value = ".")]
    root: PathBuf,
    /// Explicit trusted host executable, absolute path; not an OS sandbox.
    #[arg(long)]
    program: Option<PathBuf>,
    #[arg(long = "server-arg", allow_hyphen_values = true)]
    server_args: Vec<String>,
    #[arg(long,default_value_t=300,value_parser=clap::value_parser!(u64).range(1..=86400))]
    timeout: u64,
    /// Explicit environment variable names to pass to the trusted server.
    #[arg(long = "server-env")]
    server_env: Vec<String>,
    #[command(subcommand)]
    action: Action,
}
#[derive(Args, Clone)]
struct File {
    path: String,
    #[arg(long, default_value = "rust")]
    language: String,
}
#[derive(Args)]
struct At {
    #[command(flatten)]
    file: File,
    #[arg(long)]
    line: u32,
    #[arg(long)]
    character: u32,
}
#[derive(Subcommand)]
enum Action {
    Diagnostics {
        #[command(flatten)]
        file: File,
    },
    Definition {
        #[command(flatten)]
        at: At,
    },
    References {
        #[command(flatten)]
        at: At,
    },
    Rename {
        #[command(flatten)]
        at: At,
        #[arg(long)]
        new_name: String,
        #[arg(long)]
        apply: bool,
    },
    /// Apply one previously reviewed hash-bound plan, without starting a server.
    Apply {
        plan: String,
        #[arg(long)]
        allow_write: bool,
    },
}
#[cfg(unix)]
pub fn run(args: LspArgs, cancellation: Cancellation) -> Result<Value, AgentError> {
    let ctx = TurnContext::new(Duration::from_secs(args.timeout), cancellation)?;
    if let Action::Apply { plan, allow_write } = args.action {
        if !allow_write {
            return Err(AgentError::Configuration(
                "lsp apply requires --allow-write",
            ));
        }
        if args.program.is_some() || !args.server_args.is_empty() || !args.server_env.is_empty() {
            return Err(AgentError::Configuration(
                "offline apply does not start an LSP server",
            ));
        }
        let workspace = Workspace::new(&args.root)?;
        let plan = workspace.load_plan(&plan, &ctx)?;
        return Ok(json!({"applied":workspace.apply(&plan,&ctx)?,"model_calls":0}));
    }
    let root = args
        .root
        .canonicalize()
        .map_err(|_| AgentError::Configuration("cannot open LSP root"))?;
    let workspace = Workspace::new(&root)?;
    let file = match &args.action {
        Action::Diagnostics { file } => file,
        Action::Definition { at } | Action::References { at } | Action::Rename { at, .. } => {
            &at.file
        }
        _ => unreachable!(),
    }
    .clone();
    // Validate source path/text and UTF-16 query before launching an executable.
    let text = workspace.read(&file.path, &ctx)?;
    match &args.action {
        Action::Definition { at } | Action::References { at } | Action::Rename { at, .. } => {
            namzu_lsp::offset(
                &text,
                namzu_lsp::Position {
                    line: at.line,
                    character: at.character,
                },
            )?;
        }
        _ => (),
    }
    let mut spec = namzu_lsp::Spec::new(
        args.program.ok_or(AgentError::Configuration(
            "explicit --program required for LSP",
        ))?,
        root,
    );
    spec.args = args.server_args;
    for name in args.server_env {
        let value = std::env::var(&name).map_err(|_| {
            AgentError::Configuration("selected LSP server environment variable unavailable")
        })?;
        spec.env.insert(name, value);
    }
    let mut client = namzu_lsp::Client::start(spec, &ctx)?;
    let result = (|| {
        let doc = client.open(&file.path, &file.language, &ctx)?;
        let value = match args.action {
            Action::Diagnostics { .. } => {
                json!({"document":{"uri":doc.uri,"version":doc.version,"sha256":doc.sha256},"diagnostics":client.diagnostics(&doc.uri,&ctx)?})
            }
            Action::Definition { at } => {
                json!({"locations":client.definition(&doc.uri,namzu_lsp::Position{line:at.line,character:at.character},&ctx)?})
            }
            Action::References { at } => {
                json!({"locations":client.references(&doc.uri,namzu_lsp::Position{line:at.line,character:at.character},&ctx)?})
            }
            Action::Rename {
                at,
                new_name,
                apply,
            } => {
                let plan = client.rename(
                    &doc.uri,
                    namzu_lsp::Position {
                        line: at.line,
                        character: at.character,
                    },
                    &new_name,
                    &ctx,
                )?;
                let preview = client.workspace().preview(&plan, &ctx)?;
                if apply {
                    json!({"plan":plan,"applied":client.apply(&plan,&ctx)?})
                } else {
                    json!({"plan":plan,"preview":preview,"applied":false})
                }
            }
            _ => unreachable!(),
        };
        Ok::<Value, AgentError>(value)
    })();
    let cleanup = TurnContext::new(Duration::from_secs(2), Cancellation::default())?;
    let close = client.close(&cleanup);
    let value = result?;
    Ok(json!({"result":value,"close":close,"model_calls":0}))
}
#[cfg(not(unix))]
pub fn run(_: LspArgs, _: Cancellation) -> Result<Value, AgentError> {
    Err(AgentError::Configuration("LSP stdio requires Unix"))
}
