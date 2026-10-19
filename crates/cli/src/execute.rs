use crate::config::Config;
use clap::Args;
use namzu_core::{AgentError, Cancellation, Limits, Message, Turn, run_history_in_context};
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
    #[command(flatten)]
    pub options: RunOptions,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Clone)]
pub struct RunOptions {
    #[command(flatten)]
    pub capabilities: crate::capability_runtime::Selection,
    #[command(flatten)]
    pub computer: crate::computer_runtime::VisualSelection,
    #[arg(long, default_value = ".")]
    pub cwd: PathBuf,
    #[command(flatten)]
    pub mcp: crate::mcp_runtime::Selection,
    /// Explicit noninteractive grant for MCP connect/discovery and per-call asks.
    #[arg(long)]
    pub mcp_approve: bool,
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
    /// Explicit compatible wire format: auto, chat, responses.
    #[arg(long,value_parser=["auto","chat","responses"])]
    pub api_format: Option<String>,
    /// Request incremental API streaming; takes true or false.
    #[arg(long,action=clap::ArgAction::Set)]
    pub api_stream: Option<bool>,
    #[arg(long,value_parser=clap::value_parser!(u32).range(1..=1_000_000))]
    pub max_output_tokens: Option<u32>,
    #[arg(long,value_parser=clap::value_parser!(u64).range(1..=86400))]
    pub timeout: Option<u64>,
    #[arg(long)]
    pub max_steps: Option<usize>,
    #[arg(long)]
    pub max_tool_calls: Option<usize>,
    /// Software conversation byte budget, including opaque replay (max 4 MiB).
    #[arg(long)]
    pub max_text_bytes: Option<usize>,
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
}

pub struct Execution {
    pub session_id: Option<String>,
    pub result: Result<Turn, AgentError>,
}
pub fn execute(args: ExecArgs, cancellation: Cancellation) -> Execution {
    execute_observed(
        args.prompt,
        args.options,
        cancellation,
        &mut namzu_core::NoopEvents,
    )
}

pub fn execute_observed(
    prompt: String,
    args: RunOptions,
    cancellation: Cancellation,
    events: &mut dyn namzu_core::EventSink,
) -> Execution {
    let started = Instant::now();
    let mut session_id = None;
    let result = (|| {
        let state_dir = args.state_dir.clone().unwrap_or(default_state_dir()?);
        let store = PrivateStore::open(&state_dir)?;
        let mut config = config_for(&args, &store)?;

        let cwd = args
            .cwd
            .canonicalize()
            .map_err(|_| AgentError::Configuration("cannot open workspace"))?;
        let context = namzu_core::TurnContext::new(
            Duration::from_secs(config.timeout)
                .checked_sub(started.elapsed())
                .ok_or(AgentError::Timeout)?,
            cancellation.clone(),
        )?;
        let computer_spec = args.computer.spec()?;
        if args.computer.policy().active() && computer_spec.is_none() {
            return Err(AgentError::Configuration(
                "normal exec visual tools require --computer-image; no host fallback",
            ));
        }
        let capabilities_identity =
            crate::capability_runtime::identity(&args, &config, computer_spec.is_some())?;
        let mut provider = driver_for(&args, &config)?;
        let provider_route = provider.route(&config, &context)?;
        let mcp_identity = crate::mcp_runtime::identity(&state_dir, &args.mcp, &context)?;
        let route = if let Some(spec) = &computer_spec {
            serde_json::json!({"provider_route":serde_json::from_str::<serde_json::Value>(&provider_route).map_err(|_|AgentError::Protocol("invalid route"))?,"mcp":mcp_identity,"computer":spec}).to_string()
        } else if mcp_identity.is_null() {
            provider_route
        } else {
            serde_json::json!({"provider_route":serde_json::from_str::<serde_json::Value>(&provider_route).map_err(|_|AgentError::Protocol("invalid route"))?,"mcp":mcp_identity}).to_string()
        };
        let route = if capabilities_identity.is_null() {
            route
        } else {
            let mut value: serde_json::Value =
                serde_json::from_str(&route).map_err(|_| AgentError::Protocol("invalid route"))?;
            if value.get("provider_route").is_none() {
                value = serde_json::json!({"provider_route":value});
            }
            value["capabilities"] = capabilities_identity;
            value.to_string()
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
        let allow = match (config.tool_allow.clone(), args.tool_allow.clone()) {
            (Some(a), Some(b)) => Some(a.intersection(&b.into_iter().collect()).cloned().collect()),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b.into_iter().collect()),
            (None, None) => None,
        };
        config.tool_deny.extend(args.tool_deny.iter().cloned());
        let tool_policy = ToolPolicy {
            allow_write: args.allow_write,
            allow_shell: args.allow_shell,
            allow: allow.clone(),
            deny: config.tool_deny.clone(),
        };
        let manager = if computer_spec.is_some() {
            Some(crate::computer_runtime::manager(&state_dir, &session.id)?)
        } else {
            None
        };
        let mut shared = if let (Some(manager), Some(spec)) = (&manager, computer_spec) {
            let mut computer = if spec.sandbox.is_some()
                && manager
                    .snapshot()?
                    .is_some_and(|r| r.phase != namzu_computer::Phase::Closed)
            {
                let mut c = manager.resume(&context)?;
                c.preserve_workspace_on_drop();
                if c.snapshot().spec != spec || c.snapshot().pending.is_some() {
                    return Err(AgentError::State(
                        "sandbox changed or uncertain; inspect explicitly",
                    ));
                }
                if c.snapshot().phase == namzu_computer::Phase::Stopped {
                    c.start(&context)?;
                }
                c
            } else {
                manager.acquire(spec, &context)?
            };
            if computer.snapshot().spec.sandbox.is_some() {
                computer.preserve_workspace_on_drop();
            }
            Some(namzu_computer::SharedGuestTools::new(
                namzu_computer::GuestTools::new(computer, tool_policy.clone())
                    .with_visual(args.computer.policy()),
            ))
        } else {
            None
        };
        let mut messages = session.messages.clone();
        messages.push(Message::User { content: prompt });
        session.status = SessionStatus::Running;
        session.save(&store)?;
        let turn = (|| {
            let tools: Box<dyn namzu_core::Tools + '_> = if let Some(shared) = &shared {
                Box::new(shared.clone())
            } else {
                Box::new(
                    TerminalTools::new(&cwd, tool_policy.clone())
                        .map_err(|_| AgentError::Configuration("cannot open workspace tools"))?,
                )
            };
            let mut tools = crate::capability_runtime::tools(
                tools,
                &cwd,
                &args.capabilities,
                &config.capabilities,
                tool_policy.clone(),
            )?;
            let mut observer = PresentationObserver {
                journal: Journal {
                    store: &store,
                    session: &mut session,
                },
                events,
            };
            let limits = Limits {
                timeout: Duration::from_secs(config.timeout),
                max_steps: config.max_steps,
                max_tool_calls: config.max_tool_calls,
                max_text_bytes: config.max_text_bytes,
            };
            if args.mcp.servers.is_empty() {
                return run_history_in_context(
                    &mut provider,
                    &mut tools,
                    messages,
                    limits,
                    &context,
                    &mut observer,
                );
            }
            let registry =
                namzu_mcp::config::Registry::open(&state_dir).map_err(crate::mcp_runtime::error)?;
            let mut manager = crate::mcp_runtime::manager(&registry, &args.mcp, &context)?;
            if let Some(shared) = &shared {
                manager = manager.with_guest(shared.launcher());
            }
            let approvals = namzu_mcp::agent::Approvals::default();
            let explicit = args.mcp_approve || observer.events.supports_approval();
            let mut tools = crate::mcp_runtime::load(
                tools,
                manager,
                crate::mcp_runtime::Admission {
                    selection: &args.mcp,
                    allow: allow.as_ref(),
                    deny: &config.tool_deny,
                    explicit: args.mcp_approve,
                },
                approvals.clone(),
                observer.events,
                &context,
            )?;
            let result = run_history_in_context(
                &mut provider,
                &mut tools,
                messages,
                limits,
                &context,
                &mut namzu_mcp::agent::ApprovedObserver {
                    inner: &mut observer,
                    approvals,
                    explicit,
                },
            );
            let cleanup =
                namzu_core::TurnContext::new(Duration::from_secs(5), Cancellation::default())?;
            let closed = tools.close(&cleanup).map_err(crate::mcp_runtime::error);
            match result {
                Err(e) => Err(e),
                Ok(turn) => {
                    closed?;
                    Ok(turn)
                }
            }
        })();
        let turn = if let Some(shared) = shared.take() {
            let cleanup =
                namzu_core::TurnContext::new(Duration::from_secs(20), Cancellation::default())?;
            let cleaned = (|| {
                shared.finish_mcp(turn.is_ok(), &cleanup)?;
                let mut computer = shared.into_computer()?;
                if computer.snapshot().pending.is_some()
                    || computer.snapshot().spec.sandbox.is_some()
                {
                    computer.detach(&cleanup)?;
                } else {
                    computer.close(&cleanup)?;
                }
                Ok::<(), AgentError>(())
            })();
            match turn {
                Err(e) => Err(e),
                Ok(t) => {
                    cleaned?;
                    Ok(t)
                }
            }
        } else {
            turn
        };
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

pub(crate) enum Driver {
    Api(Box<OpenAiCompatible>),
    Codex(Box<namzu_provider::codex::CodexProvider>),
}
pub(crate) fn driver_for(args: &RunOptions, config: &Config) -> Result<Driver, AgentError> {
    let model = config.model.clone().ok_or(AgentError::Configuration(
        "model required via --model or selected config",
    ))?;
    if config.provider == "codex" {
        if args.base_url.is_some() {
            return Err(AgentError::Configuration(
                "Codex uses a fixed subscription endpoint; --base-url is for API-key providers",
            ));
        }
        Ok(Driver::Codex(Box::new(
            namzu_provider::codex::CodexProvider::new(
                args.state_dir.clone().map_or_else(default_state_dir, Ok)?,
                model,
                args.effort.clone(),
            )?,
        )))
    } else {
        let key = std::env::var(&config.api_key_env).map_err(|_| {
            AgentError::Configuration("API key environment variable is missing or invalid")
        })?;
        Ok(Driver::Api(Box::new(OpenAiCompatible::for_service(
            namzu_provider::services::Service::parse(&config.provider)?,
            &config.base_url,
            model,
            &key,
            config.api_options.clone(),
        )?)))
    }
}
pub(crate) fn api_identity(config: &Config) -> serde_json::Value {
    if config.provider == "openai-compatible" && config.api_options.is_default() {
        serde_json::json!([config.provider, config.base_url, config.model])
    } else {
        serde_json::json!({"provider":config.provider,"base_url":config.base_url,"model":config.model,"api_key_env":config.api_key_env,"api_options":config.api_options})
    }
}
impl Driver {
    pub(crate) fn route(
        &self,
        config: &Config,
        context: &namzu_core::TurnContext,
    ) -> Result<String, AgentError> {
        context.remaining()?;
        if let Self::Codex(driver) = self {
            driver.route_identity(&context.child(Duration::from_secs(30))?)
        } else {
            Ok(api_identity(config).to_string())
        }
    }
}

impl namzu_core::Provider for Driver {
    fn info(&self) -> namzu_core::ProviderInfo {
        match self {
            Self::Api(driver) => driver.info(),
            Self::Codex(driver) => driver.info(),
        }
    }
    fn take_metadata(&mut self) -> Option<namzu_core::ResponseMetadata> {
        match self {
            Self::Api(driver) => driver.take_metadata(),
            Self::Codex(driver) => driver.take_metadata(),
        }
    }

    fn take_state(&mut self) -> Option<serde_json::Value> {
        match self {
            Self::Api(driver) => driver.take_state(),
            Self::Codex(driver) => driver.take_state(),
        }
    }
    fn complete_stream(
        &mut self,
        messages: &[Message],
        tools: &[namzu_core::ToolDefinition],
        context: &namzu_core::TurnContext,
        events: &mut dyn namzu_core::EventSink,
    ) -> Result<namzu_core::Completion, AgentError> {
        match self {
            Self::Api(driver) => driver.complete_stream(messages, tools, context, events),
            Self::Codex(driver) => driver.complete_stream(messages, tools, context, events),
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

struct PresentationObserver<'a, 'b> {
    journal: Journal<'a>,
    events: &'b mut dyn namzu_core::EventSink,
}
impl namzu_core::Observer for PresentationObserver<'_, '_> {
    fn supports_approval(&self) -> bool {
        self.events.supports_approval()
    }
    fn approve(
        &mut self,
        call: &namzu_core::ToolCall,
        context: &namzu_core::TurnContext,
    ) -> Result<bool, AgentError> {
        self.events.approve(call, context)
    }
    fn checkpoint(
        &mut self,
        messages: &[Message],
        stats: &namzu_core::Stats,
        pending: Option<&namzu_core::ToolCall>,
    ) -> Result<(), AgentError> {
        namzu_core::Observer::checkpoint(&mut self.journal, messages, stats, pending)
    }
    fn event(&mut self, event: namzu_core::Event) -> Result<(), AgentError> {
        self.events.emit(event)
    }
}

pub(crate) fn config_for(args: &RunOptions, store: &PrivateStore) -> Result<Config, AgentError> {
    let mut c = Config::load(store, args.config.as_deref())?;
    if let Some(v) = &args.model {
        c.model = Some(v.clone());
    }
    if let Some(v) = &args.provider {
        if v != &c.provider && v != "codex" {
            let service = namzu_provider::services::Service::parse(v)?;
            if args.base_url.is_none() {
                c.base_url = service.base_url().into();
            }
            if args.api_key_env.is_none() {
                c.api_key_env = service.key_env().into();
            }
        }
        c.provider = v.clone();
    }
    if let Some(v) = &args.base_url {
        c.base_url = v.clone();
    }
    if let Some(v) = &args.api_key_env {
        c.api_key_env = v.clone();
    }
    if let Some(v) = args.timeout {
        c.timeout = v;
    }
    if let Some(v) = args.max_steps {
        c.max_steps = v;
    }
    if let Some(v) = args.max_tool_calls {
        c.max_tool_calls = v;
    }
    if let Some(v) = args.max_text_bytes {
        c.max_text_bytes = v;
    }
    if c.provider == "codex"
        && (args.api_format.is_some()
            || args.api_stream.is_some()
            || args.max_output_tokens.is_some())
    {
        return Err(AgentError::Configuration(
            "API wire/stream/token options do not apply to Codex",
        ));
    }
    if let Some(format) = &args.api_format {
        c.api_options.format = serde_json::from_value(serde_json::Value::String(format.clone()))
            .map_err(|_| AgentError::Configuration("invalid API wire format"))?;
    }
    if let Some(stream) = args.api_stream {
        c.api_options.stream = stream;
    }
    if let Some(max) = args.max_output_tokens {
        c.api_options.max_output_tokens = Some(max);
    }
    if c.provider != "codex"
        && let Some(effort) = &args.effort
    {
        c.api_options.reasoning_effort = Some(effort.clone());
    }
    c.validate()?;
    if c.provider != "codex" {
        let service = namzu_provider::services::Service::parse(&c.provider)?;
        if let Some(model) = &c.model {
            c.api_options.resolved(service, model)?;
        }
    }
    let names = args.capabilities.names()?;
    c.capabilities.identity(&names)?;
    if args.computer.sandbox_policy.is_some() && !c.capabilities.permits("sandbox") {
        return Err(AgentError::Configuration(
            "sandbox capability denied by config",
        ));
    }
    Ok(c)
}

#[cfg(test)]
mod service_tests {
    use super::*;
    use clap::Parser;
    #[derive(Parser)]
    struct Args {
        #[command(flatten)]
        run: RunOptions,
    }
    #[test]
    fn explicit_provider_switch_uses_service_defaults_and_optional_cli_overrides() {
        let root = tempfile::tempdir().unwrap();
        let store = PrivateStore::open(root.path()).unwrap();
        let args = Args::parse_from([
            "fixture",
            "--provider",
            "zen",
            "--model",
            "gpt-6",
            "--api-stream",
            "true",
            "--max-output-tokens",
            "100",
            "--effort",
            "low",
        ]);
        let c = config_for(&args.run, &store).unwrap();
        assert_eq!(c.base_url, "https://opencode.ai/zen/v1");
        assert_eq!(c.api_key_env, "OPENCODE_API_KEY");
        assert!(c.api_options.stream);
        assert_eq!(c.api_options.reasoning_effort.as_deref(), Some("low"));
        let args = Args::parse_from([
            "fixture",
            "--provider",
            "openrouter",
            "--base-url",
            "http://127.0.0.1:1234/v1",
            "--api-key-env",
            "FIXTURE",
        ]);
        let c = config_for(&args.run, &store).unwrap();
        assert_eq!(c.api_key_env, "FIXTURE");
        assert!(c.base_url.contains("1234"));
    }
    #[test]
    fn api_identity_pins_options_and_key_selector_with_legacy_route_compatibility() {
        let mut c = Config::default();
        assert!(api_identity(&c).is_array());
        c.provider = "openrouter".into();
        let identity = api_identity(&c);
        c.api_options.stream = true;
        assert_ne!(api_identity(&c), identity);
        c.api_options.stream = false;
        c.api_key_env = "OTHER_SELECTOR".into();
        assert_ne!(api_identity(&c), identity);
    }
    #[test]
    fn unsupported_codex_api_options_are_not_silently_ignored() {
        let root = tempfile::tempdir().unwrap();
        let store = PrivateStore::open(root.path()).unwrap();
        let args = Args::parse_from(["fixture", "--provider", "codex", "--api-stream", "false"]);
        assert!(config_for(&args.run, &store).is_err());
    }
}
