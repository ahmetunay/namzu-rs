//! Explicit operator speech commands. Not automatically exposed to model tools.
use clap::{Args, Subcommand};
use namzu_audio::{AudioEvent, AudioWorkspace};
use namzu_core::{AgentError, Cancellation, TurnContext};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
#[derive(Args)]
pub struct AudioArgs {
    #[arg(long, default_value = ".")]
    cwd: PathBuf,
    #[arg(long,default_value_t=300,value_parser=clap::value_parser!(u64).range(1..=86400))]
    timeout: u64,
    #[command(subcommand)]
    action: Action,
}
#[derive(Args)]
struct Service {
    #[arg(long, default_value = "https://api.openai.com/v1")]
    base_url: String,
    #[arg(long, default_value = "OPENAI_API_KEY")]
    api_key_env: String,
    #[arg(long)]
    model: String,
}
impl Service {
    fn driver(&self) -> Result<namzu_provider::speech::SpeechClient, AgentError> {
        let key = std::env::var(&self.api_key_env).map_err(|_| {
            AgentError::Configuration("selected speech API key environment variable missing")
        })?;
        namzu_provider::speech::SpeechClient::new(&self.base_url, &key, &self.model, &self.model)
    }
}
#[derive(Subcommand)]
enum Action {
    /// Validate a workspace PCM16 WAV locally. No credentials or network.
    Inspect { input: String },
    /// Explicitly upload a validated workspace WAV to the selected speech endpoint.
    Transcribe {
        input: String,
        #[arg(long)]
        language: Option<String>,
        #[command(flatten)]
        service: Service,
    },
    /// Explicitly send text for speech generation; publish a NEW private WAV in cwd.
    Synthesize {
        text: String,
        #[arg(long)]
        output: String,
        #[arg(long)]
        voice: String,
        #[command(flatten)]
        service: Service,
    },
}
pub fn run(args: AudioArgs, cancellation: Cancellation) -> Result<Value, AgentError> {
    let ctx = TurnContext::new(Duration::from_secs(args.timeout), cancellation)?;
    let workspace = AudioWorkspace::new(args.cwd)?;
    let mut events: Vec<AudioEvent> = vec![];
    match args.action {
        Action::Inspect { input } => {
            Ok(json!({"audio":workspace.read(&input,&ctx)?.info(),"offline":true}))
        }
        Action::Transcribe {
            input,
            language,
            service,
        } => {
            namzu_audio::validate_language(language.as_deref())?;
            let audio = workspace.read(&input, &ctx)?;
            let mut driver = service.driver()?;
            let transcript = namzu_audio::transcribe(
                &mut driver,
                &audio,
                language.as_deref(),
                &ctx,
                &mut events,
            )?;
            Ok(
                json!({"transcript":transcript,"input":audio.info(),"events":events,"offline":false}),
            )
        }
        Action::Synthesize {
            text,
            output,
            voice,
            service,
        } => {
            namzu_audio::validate_speech(&text, &voice)?;
            workspace.check_destination(&output)?;
            let mut driver = service.driver()?;
            let audio = namzu_audio::synthesize(&mut driver, &text, &voice, &ctx, &mut events)?;
            workspace.publish(&output, &audio, &ctx)?;
            Ok(json!({"output":output,"audio":audio.info(),"events":events,"offline":false}))
        }
    }
}
