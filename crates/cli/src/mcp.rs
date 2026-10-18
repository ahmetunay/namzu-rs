//! Explicit MCP management; agent execution is wired separately by mcp_runtime.
use clap::{Args as ClapArgs, Subcommand};
use namzu_core::{Cancellation, TurnContext};
use namzu_mcp::{
    config::{Decision, Error, Policy, Project, Registry, read_document, read_server},
    discovery::Kind,
    manager::{Manager, Outcome, Plan},
};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
#[derive(ClapArgs)]
pub struct Args {
    #[arg(long)]
    state_dir: Option<PathBuf>,
    #[arg(long)]
    profile: Option<String>,
    /// Canonical project scope; never automatically read from cwd.
    #[arg(long, requires = "project_config")]
    project: Option<PathBuf>,
    /// Explicit relative JSON path inside --project.
    #[arg(long, requires = "project")]
    project_config: Option<PathBuf>,
    /// Additional operation ceiling; comma separated exact methods/targets or method:*.
    #[arg(long, value_delimiter = ',')]
    allow: Option<Vec<String>>,
    #[arg(long, value_delimiter = ',')]
    deny: Vec<String>,
    /// Require this invocation's explicit --approve even if stored policy allows.
    #[arg(long)]
    require_approval: bool,
    #[command(subcommand)]
    action: Action,
}
#[derive(Subcommand)]
enum Action {
    /// Explicit OAuth management; agent calls never start interactive login.
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
    Init,
    /// Validate and atomically replace private user profiles. Does not launch anything.
    Import {
        file: PathBuf,
    },
    List,
    /// Add a new server from an explicit JSON Server spec, without launching it.
    Add {
        server: String,
        #[arg(long)]
        spec: PathBuf,
    },
    /// Replace an existing server definition, without launching it.
    Update {
        server: String,
        #[arg(long)]
        spec: PathBuf,
    },
    /// Redacted status: does not print argv, endpoint, env selectors or remote content.
    Show {
        server: String,
    },
    Enable {
        server: String,
    },
    Disable {
        server: String,
    },
    Remove {
        server: String,
    },
    Profile {
        #[command(subcommand)]
        action: ProfileAction,
    },
    /// Trust exactly the current explicitly selected project config bytes.
    Trust,
    Untrust,
    /// Fetch one complete bounded MCP catalog. Does not call tools/read resources.
    Discover {
        server: String,
        #[arg(value_parser=["tools","resources","prompts","templates"])]
        kind: String,
        /// Approve connect and each page in this invocation, never overriding deny.
        #[arg(long)]
        approve: bool,
        /// Include untrusted definitions/descriptions/URI/schema data in JSON output.
        #[arg(long)]
        details: bool,
        /// Print stable model function aliases for Pal/CLI tool allowlists.
        #[arg(long)]
        agent_names: bool,
        #[arg(long,default_value_t=300,value_parser=clap::value_parser!(u64).range(1..=300))]
        timeout: u64,
    },
    /// Explicit initialize + ping, then close. No model/tool calls or downloads.
    Check {
        server: String,
        /// Approve only this invocation's connect and ping if policy asks. Cannot override deny.
        #[arg(long)]
        approve: bool,
        #[arg(long,default_value_t=300,value_parser=clap::value_parser!(u64).range(1..=300))]
        timeout: u64,
    },
}
#[derive(Subcommand)]
enum AuthAction {
    Login {
        server: String,
        #[arg(long)]
        approve: bool,
        /// Loopback callback port; zero chooses an available port.
        #[arg(long, default_value_t = 0)]
        port: u16,
        #[arg(long, default_value_t=300, value_parser=clap::value_parser!(u64).range(1..=300))]
        timeout: u64,
    },
    Status {
        server: String,
    },
    /// Delete local credentials; does not revoke the provider's token remotely.
    Logout {
        server: String,
    },
}
#[derive(Subcommand)]
enum ProfileAction {
    List,
    Create { name: String },
    Default { name: String },
    Remove { name: String },
}
fn admitted(plan: Plan, approve: bool) -> Result<namzu_mcp::manager::Permit, Error> {
    if approve && plan.decision() == Decision::Ask {
        plan.approve()
    } else {
        plan.permit()
    }
}
pub fn run(args: Args, cancellation: Cancellation) -> Result<Value, Error> {
    let state = args
        .state_dir
        .unwrap_or(namzu_session::default_state_dir()?);
    let registry = Registry::open(&state)?;
    let project = match (&args.project, &args.project_config) {
        (Some(root), Some(config)) => Some(Project::new(root, config)?),
        (None, None) => None,
        _ => return Err(Error::Configuration),
    };
    let ceiling = Policy {
        allow: args.allow.map(|v| v.into_iter().collect()),
        deny: args.deny.into_iter().collect(),
        require_approval: args.require_approval,
    }; // host ceiling cannot relax server/project policies
    if project.is_some()
        && !matches!(
            &args.action,
            Action::Trust
                | Action::Untrust
                | Action::List
                | Action::Show { .. }
                | Action::Check { .. }
                | Action::Discover { .. }
                | Action::Auth { .. }
        )
    {
        return Err(Error::Configuration);
    }
    let profile = args.profile.as_deref();
    match args.action {
        Action::Auth { action } => match action {
            AuthAction::Login {
                server,
                approve,
                port,
                timeout,
            } => {
                let ctx = TurnContext::new(Duration::from_secs(timeout), cancellation)?;
                registry.oauth_login(
                    namzu_mcp::config::OAuthLogin {
                        profile,
                        project: project.as_ref(),
                        server: &server,
                        ceiling,
                        approve,
                        port,
                    },
                    &ctx,
                    |url| {
                        use std::io::Write;
                        eprintln!("Open this authorization URL in your browser:\n{url}");
                        let _ = std::io::stderr().flush();
                    },
                )?;
                Ok(json!({"logged_in":true,"model_calls":0}))
            }
            AuthAction::Status { server } => Ok(json!(registry.oauth_status(
                profile,
                project.as_ref(),
                &server,
                false
            )?)),
            AuthAction::Logout { server } => {
                registry.oauth_status(profile, project.as_ref(), &server, true)?;
                Ok(json!({"logged_out":true,"remote_revoked":false}))
            }
        },
        Action::Init => {
            registry.init()?;
            Ok(json!({"initialized":true}))
        }
        Action::Import { file } => {
            registry.replace(read_document(&file)?)?;
            Ok(json!({"imported":true}))
        }
        Action::Add { server, spec } => {
            registry.put_server(profile, &server, read_server(&spec)?, false)?;
            Ok(json!({"added":true}))
        }
        Action::Update { server, spec } => {
            registry.put_server(profile, &server, read_server(&spec)?, true)?;
            Ok(json!({"updated":true}))
        }
        Action::List => Ok(json!(registry.list(profile, project.as_ref(), &ceiling)?)),
        Action::Show { server } => Ok(json!(
            registry
                .list(profile, project.as_ref(), &ceiling)?
                .into_iter()
                .find(|s| s.name == server)
                .ok_or(Error::NotFound)?
        )),
        Action::Enable { server } | Action::Disable { server } | Action::Remove { server }
            if project.is_some() =>
        {
            let _ = server;
            Err(Error::Configuration)
        }
        Action::Enable { server } => {
            registry.edit_server(profile, &server, Some(true))?;
            Ok(json!({"enabled":true}))
        }
        Action::Disable { server } => {
            registry.edit_server(profile, &server, Some(false))?;
            Ok(json!({"enabled":false}))
        }
        Action::Remove { server } => {
            registry.edit_server(profile, &server, None)?;
            Ok(json!({"removed":true}))
        }
        Action::Profile { action } => match action {
            ProfileAction::List => {
                let (default, profiles) = registry.profiles()?;
                Ok(json!({"default":default,"profiles":profiles}))
            }
            ProfileAction::Create { name } => {
                registry.create_profile(&name)?;
                Ok(json!({"created":true}))
            }
            ProfileAction::Default { name } => {
                registry.default_profile(&name)?;
                Ok(json!({"default":name}))
            }
            ProfileAction::Remove { name } => {
                registry.remove_profile(&name)?;
                Ok(json!({"removed":true}))
            }
        },
        Action::Trust => Ok(
            json!({"trusted_digest":registry.trust(project.as_ref().ok_or(Error::Configuration)?)?}),
        ),
        Action::Untrust => {
            registry.untrust(project.as_ref().ok_or(Error::Configuration)?)?;
            Ok(json!({"trusted":false}))
        }
        Action::Discover {
            server,
            kind,
            approve,
            details,
            agent_names,
            timeout,
        } => {
            let kind = match kind.as_str() {
                "tools" => Kind::Tools,
                "resources" => Kind::Resources,
                "prompts" => Kind::Prompts,
                "templates" => Kind::Templates,
                _ => return Err(Error::Configuration),
            };
            let context = TurnContext::new(Duration::from_secs(timeout), cancellation)?;
            let mut manager = Manager::new(&registry, args.profile, project, ceiling, &context)?;
            let result = (|| {
                let permission = admitted(manager.prepare_connect(&server)?, approve)?;
                manager.execute(permission, &context)?;
                manager.discover(&server, kind, &context, |plan| admitted(plan, approve))
            })();
            let cleanup = TurnContext::new(Duration::from_secs(5), Cancellation::default())?;
            let closed = manager.close(&cleanup);
            let catalog = result?;
            closed?;
            if agent_names {
                Ok(
                    json!({"server":server,"kind":kind,"items":catalog.items.iter().map(|item|json!({"alias":namzu_mcp::agent::alias(&server,kind,item.identity()),"identity":item.identity()})).collect::<Vec<_>>()}),
                )
            } else if details {
                Ok(json!(catalog))
            } else {
                Ok(
                    json!({"server":server,"kind":kind,"count":catalog.items.len(),"items":catalog.items.iter().map(|item|item.identity()).collect::<Vec<_>>(),"model_calls":0}),
                )
            }
        }
        Action::Check {
            server,
            approve,
            timeout,
        } => {
            let context = TurnContext::new(Duration::from_secs(timeout), cancellation)?;
            let mut manager = Manager::new(&registry, args.profile, project, ceiling, &context)?;
            let result = (|| {
                let permission = admitted(manager.prepare_connect(&server)?, approve)?;
                manager.execute(permission, &context)?;
                let permission = admitted(manager.prepare_call(&server, "ping", None)?, approve)?;
                match manager.execute(permission, &context)? {
                    Outcome::Reply(Ok(_)) => Ok(()),
                    _ => Err(Error::Configuration),
                }
            })();
            let cleanup = TurnContext::new(Duration::from_secs(5), Cancellation::default())?;
            let closed = manager.close(&cleanup);
            result?;
            closed?;
            Ok(json!({"server":server,"connected":true,"ping":true,"closed":true,"model_calls":0}))
        }
    }
}
