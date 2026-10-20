use crate::execute::{Driver, RunOptions, config_for_discovery, driver_for};
use clap::Args;
use namzu_core::{AgentError, Cancellation, TurnContext};
use namzu_session::{PrivateStore, default_state_dir};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
#[derive(Args)]
pub struct ModelsArgs {
    #[arg(long)]
    pub state_dir: Option<PathBuf>,
    #[arg(long)]
    pub provider: Option<String>,
    #[arg(long)]
    pub base_url: Option<String>,
    #[arg(long)]
    pub api_key_env: Option<String>,
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[arg(long)]
    pub aws_region: Option<String>,
    #[arg(long,value_parser=["bearer","sigv4"])]
    pub bedrock_auth: Option<String>,
    #[arg(long,value_parser=clap::value_parser!(u64).range(1..=300))]
    pub timeout: Option<u64>,
    /// Print one exact ID/alias, never choose a replacement model.
    #[arg(long)]
    pub model: Option<String>,
    /// Atomically save a private immutable catalogue and return its selection ID.
    #[arg(long, conflicts_with = "cached")]
    pub save: bool,
    /// Inspect a saved catalogue offline; no credentials or network are read.
    #[arg(long,conflicts_with_all=["provider","base_url","api_key_env","config","aws_region","bedrock_auth","timeout"])]
    pub cached: Option<String>,
}
pub fn run(args: ModelsArgs, cancel: Cancellation) -> Result<Value, AgentError> {
    let state = args.state_dir.clone().map_or_else(default_state_dir, Ok)?;
    let store = PrivateStore::open(&state)?;
    if let Some(id) = &args.cached {
        let cached = namzu_provider::catalog::Cached::read(&store, id)?;
        let mut catalog = cached.catalog;
        if let Some(model) = &args.model {
            catalog.models = vec![catalog.select(model)?.clone()];
        }
        return Ok(
            json!({"format_version":1,"status":"ok","catalog_id":id,"catalog":catalog,"offline":true}),
        );
    }
    let legacy = args.provider.is_none()
        && args.config.is_none()
        && args.base_url.is_none()
        && args.api_key_env.is_none()
        && args.aws_region.is_none()
        && args.bedrock_auth.is_none()
        && args.timeout.is_none()
        && args.model.is_none()
        && !args.save;
    let context = TurnContext::new(Duration::from_secs(args.timeout.unwrap_or(30)), cancel)?;
    if legacy {
        return namzu_provider::codex::CodexProvider::new(state, "catalogue".into(), None)?
            .models(&context);
    }
    let options = RunOptions {
        provider: args
            .provider
            .or_else(|| args.config.is_none().then(|| "codex".into())),
        base_url: args.base_url,
        api_key_env: args.api_key_env,
        config: args.config,
        state_dir: Some(state),
        aws_region: args.aws_region,
        bedrock_auth: args.bedrock_auth,
        cwd: PathBuf::from("."),
        ..Default::default()
    };
    let config = config_for_discovery(&options, &store)?;
    let driver = driver_for(&options, &config)?;
    let mut catalog = match &driver {
        Driver::Api(p) => p.discover_models(&context)?,
        Driver::Codex(p) => p.discover_models(&context)?,
    };
    let selected = args
        .model
        .as_ref()
        .map(|id| catalog.select(id).cloned())
        .transpose()?;
    context.remaining()?;
    let id = if args.save {
        Some(match &driver {
            Driver::Api(p) => p.save_catalog(&store, catalog.clone(), &config.api_key_env)?,
            Driver::Codex(p) => p.save_catalog(&store, catalog.clone(), &context)?,
        })
    } else {
        None
    };
    if let Some(model) = selected {
        catalog.models = vec![model];
    }
    Ok(json!({"format_version":1,"status":"ok","catalog_id":id,"catalog":catalog,"offline":false}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[derive(Parser)]
    struct Args {
        #[command(flatten)]
        run: RunOptions,
    }
    #[test]
    fn catalogue_config_checks_exact_model_route_and_explicit_output_before_transport() {
        let dir = tempfile::tempdir().unwrap();
        let store = PrivateStore::open(dir.path().join("state")).unwrap();
        let provider = namzu_provider::OpenAiCompatible::for_service(
            namzu_provider::services::Service::Openrouter,
            "http://127.0.0.1:1",
            "fixture".into(),
            "fixture-key",
            Default::default(),
        )
        .unwrap();
        let catalog = namzu_provider::catalog::Catalog {
            version: 1,
            provider: "openrouter".into(),
            base_url: "http://127.0.0.1:1/".into(),
            captured_ms: 1,
            models: vec![namzu_provider::catalog::Model {
                id: "fixture".into(),
                output_token_limit: Some(100),
                ..Default::default()
            }],
        };
        let id = provider
            .save_catalog(&store, catalog, "FIXTURE_KEY")
            .unwrap();
        let args = Args::parse_from([
            "fixture",
            "--provider",
            "openrouter",
            "--base-url",
            "http://127.0.0.1:1",
            "--api-key-env",
            "FIXTURE_KEY",
            "--model",
            "fixture",
            "--model-catalog",
            &id,
        ]);
        let c = crate::execute::config_for(&args.run, &store).unwrap();
        assert_eq!(c.api_options.max_output_tokens, Some(100));
        let mut changed = args.run.clone();
        changed.max_output_tokens = Some(101);
        assert!(crate::execute::config_for(&changed, &store).is_err());
        let mut changed = args.run.clone();
        changed.model = Some("other-paid-model".into());
        assert!(crate::execute::config_for(&changed, &store).is_err());
        let mut changed = args.run.clone();
        changed.api_key_env = Some("OTHER_SELECTOR".into());
        assert!(crate::execute::config_for(&changed, &store).is_err());
        let mut changed = args.run;
        changed.model = None;
        assert!(
            crate::execute::config_for(&changed, &store)
                .unwrap()
                .model
                .is_none()
        );
    }
}
