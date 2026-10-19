//! Shared exec/TUI/Pal capability selection. No eager server, login, or speech request.
use clap::{Args, Subcommand};
use namzu_capabilities::{AudioConfig, CapabilityTools};
use namzu_core::{AgentError, Cancellation, Tools, TurnContext};
use namzu_tools::{TerminalTools, ToolPolicy};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::Duration,
};
#[derive(Args, Clone, Default)]
#[group(id = "capability_selection")]
pub struct Selection {
    /// Optional packages, explicitly selected on every invocation: audio,lsp.
    #[arg(long = "capability", value_delimiter = ',')]
    pub packages: Vec<String>,
    /// Permit speech service requests (WAV upload/text transmission) this invocation.
    #[arg(long)]
    pub allow_audio: bool,
    /// Permit the configured host LSP executable this invocation. Not an OS sandbox.
    #[arg(long)]
    pub allow_lsp: bool,
}
impl Selection {
    pub fn names(&self) -> Result<BTreeSet<String>, AgentError> {
        let names: BTreeSet<_> = self.packages.iter().cloned().collect();
        if names.len() != self.packages.len()
            || names.len() > 2
            || names
                .iter()
                .any(|s| !["audio", "lsp"].contains(&s.as_str()))
            || self.allow_audio && !names.contains("audio")
            || self.allow_lsp && !names.contains("lsp")
        {
            return Err(AgentError::Configuration(
                "invalid explicit capability selection or grant",
            ));
        }
        Ok(names)
    }
}
struct LazySpeech(AudioConfig);
impl LazySpeech {
    fn driver(&self) -> Result<namzu_provider::speech::SpeechClient, AgentError> {
        let key = std::env::var(&self.0.api_key_env).map_err(|_| {
            AgentError::Configuration("selected speech API key environment variable unavailable")
        })?;
        namzu_provider::speech::SpeechClient::new(
            &self.0.base_url,
            &key,
            &self.0.transcription_model,
            &self.0.speech_model,
        )
    }
}
impl namzu_audio::SpeechInput for LazySpeech {
    fn transcribe(
        &mut self,
        a: &namzu_audio::Wav,
        l: Option<&str>,
        c: &TurnContext,
    ) -> Result<namzu_audio::Transcript, AgentError> {
        namzu_audio::SpeechInput::transcribe(&mut self.driver()?, a, l, c)
    }
}
impl namzu_audio::SpeechOutput for LazySpeech {
    fn synthesize(
        &mut self,
        t: &str,
        v: &str,
        c: &TurnContext,
    ) -> Result<namzu_audio::Wav, AgentError> {
        namzu_audio::SpeechOutput::synthesize(&mut self.driver()?, t, v, c)
    }
}
pub fn identity(
    options: &crate::execute::RunOptions,
    config: &crate::config::Config,
    guest: bool,
) -> Result<Value, AgentError> {
    config
        .capabilities
        .registry(&options.capabilities.names()?, guest)?;
    if options.computer.sandbox_policy.is_some() && !config.capabilities.permits("sandbox") {
        return Err(AgentError::Configuration(
            "sandbox capability denied by config",
        ));
    }
    config.capabilities.identity(&options.capabilities.names()?)
}
pub fn tools<L: Tools>(
    local: L,
    root: &Path,
    selection: &Selection,
    config: &namzu_capabilities::Config,
    policy: ToolPolicy,
) -> Result<CapabilityTools<L>, AgentError> {
    let mut tools = CapabilityTools::new(local, root, config.clone(), selection.names()?, policy)?
        .grants(selection.allow_audio, selection.allow_lsp);
    if let Some(audio) = &config.audio {
        tools = tools.with_speech(Box::new(LazySpeech(audio.clone())));
    }
    Ok(tools)
}
#[derive(Args)]
pub struct CapabilityArgs {
    #[command(flatten)]
    selection: Selection,
    #[command(flatten)]
    computer: crate::computer_runtime::VisualSelection,
    #[arg(long, default_value = ".")]
    cwd: PathBuf,
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    state_dir: Option<PathBuf>,
    #[arg(long)]
    allow_write: bool,
    #[arg(long)]
    allow_shell: bool,
    #[arg(long, value_delimiter = ',')]
    tool_allow: Option<Vec<String>>,
    #[arg(long, value_delimiter = ',')]
    tool_deny: Vec<String>,
    #[command(subcommand)]
    action: Option<Action>,
}
#[derive(Subcommand)]
enum Action {
    List,
    Show { id: String },
    Cleanup { id: String },
}
pub fn inspect(args: CapabilityArgs, cancellation: Cancellation) -> Result<Value, AgentError> {
    let state = args
        .state_dir
        .map_or_else(namzu_session::default_state_dir, Ok)?;
    if let Some(Action::Show { id } | Action::Cleanup { id }) = &args.action {
        let manager = crate::computer_runtime::manager(&state, id)?;
        if matches!(args.action, Some(Action::Cleanup { .. })) {
            manager.recover_cleanup(&TurnContext::new(Duration::from_secs(20), cancellation)?)?;
        }
        return Ok(json!({"session_id":id,"computer":manager.snapshot()?}));
    }
    let config = crate::config::Config::load(
        &namzu_session::PrivateStore::open(state)?,
        args.config.as_deref(),
    )?;
    let names = args.selection.names()?;
    let spec = args.computer.spec()?;
    if args.computer.sandbox_policy.is_some() && !config.capabilities.permits("sandbox") {
        return Err(AgentError::Configuration("sandbox capability denied"));
    }
    let registry = config.capabilities.registry(&names, spec.is_some())?;
    let mut policy = ToolPolicy {
        allow_write: args.allow_write,
        allow_shell: args.allow_shell,
        allow: match (
            config.tool_allow,
            args.tool_allow
                .map(|v| v.into_iter().collect::<BTreeSet<_>>()),
        ) {
            (Some(a), Some(b)) => Some(a.intersection(&b).cloned().collect()),
            (a, b) => a.or(b),
        },
        deny: config.tool_deny,
    };
    policy.deny.extend(args.tool_deny);
    let tool_names: Vec<String> = if let Some(spec) = &spec {
        ["read_file", "list_files", "write_file", "shell"]
            .into_iter()
            .filter(|n| {
                config.capabilities.permits("files")
                    && !policy.deny.contains(*n)
                    && policy.allow.as_ref().is_none_or(|a| a.contains(*n))
                    && match *n {
                        "write_file" => {
                            policy.allow_write && spec.sandbox.as_ref().is_none_or(|p| p.writable())
                        }
                        "shell" => {
                            policy.allow_shell && spec.sandbox.as_ref().is_none_or(|p| p.commands())
                        }
                        _ => true,
                    }
            })
            .map(String::from)
            .collect()
    } else {
        tools(
            TerminalTools::new(&args.cwd, policy.clone())
                .map_err(|_| AgentError::Configuration("cannot open capability workspace"))?,
            &args.cwd,
            &args.selection,
            &config.capabilities,
            policy,
        )?
        .definitions()
        .into_iter()
        .map(|d| d.name)
        .collect()
    };
    Ok(
        json!({"registry":registry,"tools":tool_names,"computer":spec,"grants":{"audio":args.selection.allow_audio,"lsp":args.selection.allow_lsp},"model_calls":0,"servers_started":0}),
    )
}
