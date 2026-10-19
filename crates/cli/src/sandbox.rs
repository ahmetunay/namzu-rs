//! Explicit offline operator commands; no model, login or automatic host fallback.
use clap::{Args, Subcommand, ValueEnum};
use namzu_computer::{ComputerManager, ComputerSpec, Docker};
use namzu_core::{AgentError, Cancellation, TurnContext};
use namzu_sandbox::Policy;
use namzu_session::PrivateStore;
use serde_json::{Value, json};
use std::{io::Read, path::PathBuf, time::Duration};
#[derive(Args)]
pub struct SandboxArgs {
    #[arg(long)]
    state_dir: Option<PathBuf>,
    #[command(subcommand)]
    action: Action,
}
#[derive(Clone, Copy, ValueEnum)]
enum Profile {
    Inspect,
    Readonly,
    Build,
}
#[derive(Args)]
struct Selection {
    #[arg(long)]
    policy: PathBuf,
    /// Prebuilt local guest image ID, sha256:...; never pulls or builds.
    #[arg(long)]
    image: String,
}
#[derive(Subcommand)]
enum Action {
    Capabilities,
    Policy {
        #[arg(long, value_enum, default_value = "inspect")]
        profile: Profile,
    },
    Check {
        policy: PathBuf,
    },
    Run {
        #[command(flatten)]
        selection: Selection,
        command: String,
    },
    Read {
        #[command(flatten)]
        selection: Selection,
        path: String,
    },
    List {
        #[command(flatten)]
        selection: Selection,
        #[arg(default_value = ".")]
        path: String,
    },
    Show {
        id: String,
    },
    Cleanup {
        id: String,
    },
    /// Read a retained workspace without replaying or acknowledging uncertain effects.
    ReadRetained {
        id: String,
        path: String,
    },
}
fn load(path: PathBuf) -> Result<Policy, AgentError> {
    let file = std::fs::File::open(path)
        .map_err(|_| AgentError::Configuration("cannot open selected sandbox policy"))?;
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| AgentError::Configuration("cannot read sandbox policy"))?;
    Policy::parse(&bytes)
}
fn manager(root: &std::path::Path, id: &str) -> Result<ComputerManager, AgentError> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        return Err(AgentError::Configuration("invalid sandbox id"));
    }
    ComputerManager::from_store(
        PrivateStore::open(root.join("sandboxes").join(id))?,
        "sandbox",
        id,
        Docker::local("/usr/local/bin/docker")?,
    )
}
pub fn run(args: SandboxArgs, cancellation: Cancellation) -> Result<Value, AgentError> {
    match args.action {
        Action::Capabilities => {
            return Ok(
                json!({"implemented":["local_docker"],"native":false,"network":"disabled","host_fallback":false,"visual_profiles":false,"model_calls":0}),
            );
        }
        Action::Policy { profile } => {
            let policy = match profile {
                Profile::Inspect => Policy::inspect(),
                Profile::Readonly => Policy::readonly(),
                Profile::Build => Policy::build(),
            };
            return Ok(json!({"sha256":policy.fingerprint()?,"policy":policy}));
        }
        Action::Check { policy } => {
            let policy = load(policy)?;
            return Ok(json!({"sha256":policy.fingerprint()?,"policy":policy}));
        }
        _ => (),
    }
    let root = match args.state_dir {
        Some(path) => path,
        None => namzu_session::default_state_dir()?,
    };
    let ctx = TurnContext::new(Duration::from_secs(300), cancellation)?;
    match args.action {
        Action::Show { id } => {
            Ok(json!({"sandbox_id":id,"computer":manager(&root,&id)?.snapshot()?}))
        }
        Action::ReadRetained { id, path } => {
            if !namzu_computer::protocol::valid_path(&path) {
                return Err(AgentError::Configuration("invalid sandbox path"));
            }
            let manager = manager(&root, &id)?;
            let mut computer = manager.resume(&ctx)?;
            computer.preserve_workspace_on_drop();
            if computer.snapshot().phase == namzu_computer::Phase::Stopped {
                computer.start(&ctx)?;
            }
            use base64::Engine;
            let bytes = computer.download(&path, &ctx)?;
            let cleanup_ctx = TurnContext::new(Duration::from_secs(20), Cancellation::default())?;
            let record = computer.detach(&cleanup_ctx)?;
            Ok(
                json!({"sandbox_id":id,"base64":base64::engine::general_purpose::STANDARD.encode(bytes),"computer":record}),
            )
        }
        Action::Cleanup { id } => {
            let manager = manager(&root, &id)?;
            manager.recover_cleanup(&ctx)?;
            Ok(json!({"sandbox_id":id,"computer":manager.snapshot()?,"cleaned":true}))
        }
        action => {
            let (selection, operation, argument) = match action {
                Action::Run { selection, command } => (selection, "run", command),
                Action::Read { selection, path } => (selection, "read", path),
                Action::List { selection, path } => (selection, "list", path),
                _ => unreachable!(),
            };
            let policy = load(selection.policy)?;
            if operation == "run" {
                policy.check_command()?;
                if argument.is_empty() || argument.len() > 65536 {
                    return Err(AgentError::Configuration("invalid sandbox command"));
                }
            } else if !namzu_computer::protocol::valid_path(&argument) {
                return Err(AgentError::Configuration("invalid sandbox path"));
            }
            let spec = ComputerSpec::new(selection.image).with_sandbox(policy)?;
            let id = namzu_session::new_id();
            let manager = manager(&root, &id)?;
            // Always include the recovery identity, even if acquisition or execution fails.
            let result: Result<Value, AgentError> = (|| {
                let mut computer = manager.acquire(spec, &ctx)?;
                match operation {
                    "run" => Ok(json!(computer.shell(
                        &argument,
                        ".",
                        Duration::from_secs(300),
                        &ctx
                    )?)),
                    "read" => {
                        use base64::Engine;
                        Ok(
                            json!({"base64":base64::engine::general_purpose::STANDARD.encode(computer.download(&argument,&ctx)?)}),
                        )
                    }
                    _ => Ok(json!(computer.list(&argument, &ctx)?)),
                }
                // Drop cleans healthy resources, stops and retains uncertain ones.
            })();
            let (result, error) = match result {
                Ok(value) => (Some(value), None),
                Err(e) => (None, Some(e.to_string())),
            };
            let record = manager.snapshot()?;
            let cleaned = record
                .as_ref()
                .is_some_and(|r| r.phase == namzu_computer::Phase::Closed);
            Ok(
                json!({"sandbox_id":id,"result":result,"error":error,"computer":record,"cleaned":cleaned,"model_calls":0}),
            )
        }
    }
}
