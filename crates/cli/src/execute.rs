use crate::config::Config;
use clap::Args;
use namzu_core::{AgentError, Cancellation, Limits, Message, Turn, run_history};
use namzu_provider::OpenAiCompatible;
use namzu_session::{Journal, PrivateStore, Session, SessionStatus, default_state_dir, new_id};
use namzu_tools::{TerminalTools, ToolPolicy};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Args)]
pub struct ExecArgs {
    pub prompt: String,
    #[arg(long, default_value = ".")]
    pub cwd: PathBuf,
    #[arg(long)]
    pub model: Option<String>,
    #[arg(long)]
    pub provider: Option<String>,
    #[arg(long)]
    pub base_url: Option<String>,
    #[arg(long)]
    pub api_key_env: Option<String>,
    #[arg(long)]
    pub effort: Option<String>,
    #[arg(long,value_parser=clap::value_parser!(u64).range(1..=86400))]
    pub timeout: Option<u64>,
    #[arg(long)]
    pub max_steps: Option<usize>,
    #[arg(long)]
    pub max_tool_calls: Option<usize>,
    #[arg(long)]
    pub allow_write: bool,
    /// Grant host shell execution, including host files/network outside cwd.
    #[arg(long)]
    pub allow_shell: bool,
    #[arg(long, value_delimiter = ',')]
    pub tool_allow: Option<Vec<String>>,
    #[arg(long, value_delimiter = ',')]
    pub tool_deny: Vec<String>,
    /// Explicitly trust/select a config file; project config is never auto-loaded.
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[arg(long)]
    pub state_dir: Option<PathBuf>,
    /// Continue a completed session; failed/interrupted sessions cannot auto-replay.
    #[arg(long)]
    pub resume: Option<String>,
    #[arg(long)]
    pub json: bool,
}

pub struct Execution {
    pub session_id: Option<String>,
    pub result: Result<Turn, AgentError>,
}
pub fn execute(args: ExecArgs, cancellation: Cancellation) -> Execution {
    let started = Instant::now();
    let mut session_id = None;
    let result = (|| {
        let state_dir = args.state_dir.clone().unwrap_or(default_state_dir()?);
        let explicit_base_url = args.base_url.is_some();
        let store = PrivateStore::open(&state_dir)?;
        let mut config = Config::load(&store, args.config.as_deref())?;
        if let Some(value) = args.model {
            config.model = Some(value);
        }
        if let Some(value) = args.provider {
            config.provider = value;
        }
        if let Some(value) = args.base_url {
            config.base_url = value;
        }
        if let Some(value) = args.api_key_env {
            config.api_key_env = value;
        }
        if let Some(value) = args.timeout {
            config.timeout = value;
        }
        if let Some(value) = args.max_steps {
            config.max_steps = value;
        }
        if let Some(value) = args.max_tool_calls {
            config.max_tool_calls = value;
        }
        config.validate()?;
        let model = config.model.clone().ok_or(AgentError::Configuration(
            "model required via --model or selected config",
        ))?;
        let mut provider = if config.provider == "codex" {
            if explicit_base_url {
                return Err(AgentError::Configuration(
                    "Codex uses a fixed subscription endpoint; --base-url is for API-key providers",
                ));
            }
            Driver::Codex(Box::new(namzu_provider::codex::CodexProvider::new(
                state_dir,
                model.clone(),
                args.effort,
            )?))
        } else {
            if args.effort.is_some() {
                return Err(AgentError::Configuration(
                    "--effort currently applies to Codex only",
                ));
            }
            let key = std::env::var(&config.api_key_env).map_err(|_| {
                AgentError::Configuration("API key environment variable is missing or invalid")
            })?;
            Driver::Api(Box::new(OpenAiCompatible::new(
                &config.base_url,
                model.clone(),
                &key,
            )?))
        };
        let cwd = args
            .cwd
            .canonicalize()
            .map_err(|_| AgentError::Configuration("cannot open workspace"))?;
        let route = if let Driver::Codex(driver) = &provider {
            driver.route_identity(&namzu_core::TurnContext::new(
                Duration::from_secs(config.timeout)
                    .checked_sub(started.elapsed())
                    .ok_or(AgentError::Timeout)?
                    .min(Duration::from_secs(30)),
                cancellation.clone(),
            )?)?
        } else {
            serde_json::to_string(&(&config.provider, &config.base_url, &model))
                .map_err(|_| AgentError::Configuration("cannot encode provider route"))?
        };
        let id = args.resume.clone().unwrap_or_else(new_id);
        let _lock = store.lock(&id)?;
        let mut session = if args.resume.is_some() {
            let session = Session::load(&store, &id)?;
            session.require_resumable(&cwd, &route)?;
            session
        } else {
            Session::new(id.clone(), cwd.clone(), route)?
        };
        session_id = Some(id);
        let allow = match (config.tool_allow, args.tool_allow) {
            (Some(a), Some(b)) => Some(a.intersection(&b.into_iter().collect()).cloned().collect()),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b.into_iter().collect()),
            (None, None) => None,
        };
        config.tool_deny.extend(args.tool_deny);
        let mut tools = TerminalTools::new(
            &cwd,
            ToolPolicy {
                allow_write: args.allow_write,
                allow_shell: args.allow_shell,
                allow,
                deny: config.tool_deny,
            },
        )
        .map_err(|_| AgentError::Configuration("cannot open workspace tools"))?;
        let mut messages = session.messages.clone();
        messages.push(Message::User {
            content: args.prompt,
        });
        session.status = SessionStatus::Running;
        session.save(&store)?;
        let turn = run_history(
            &mut provider,
            &mut tools,
            messages,
            Limits {
                timeout: Duration::from_secs(config.timeout)
                    .checked_sub(started.elapsed())
                    .filter(|value| !value.is_zero())
                    .ok_or(AgentError::Timeout)?,
                max_steps: config.max_steps,
                max_tool_calls: config.max_tool_calls,
                ..Limits::default()
            },
            cancellation,
            &mut Journal {
                store: &store,
                session: &mut session,
            },
        );
        session.status = if turn.is_ok() {
            SessionStatus::Complete
        } else {
            SessionStatus::Failed
        };
        session.save(&store)?;
        turn
    })();
    Execution { session_id, result }
}

enum Driver {
    Api(Box<OpenAiCompatible>),
    Codex(Box<namzu_provider::codex::CodexProvider>),
}
impl namzu_core::Provider for Driver {
    fn take_state(&mut self) -> Option<serde_json::Value> {
        match self {
            Self::Api(driver) => driver.take_state(),
            Self::Codex(driver) => driver.take_state(),
        }
    }
    fn complete(
        &mut self,
        messages: &[Message],
        tools: &[namzu_core::ToolDefinition],
        context: &namzu_core::TurnContext,
    ) -> Result<namzu_core::Completion, AgentError> {
        match self {
            Self::Api(driver) => driver.complete(messages, tools, context),
            Self::Codex(driver) => driver.complete(messages, tools, context),
        }
    }
}
