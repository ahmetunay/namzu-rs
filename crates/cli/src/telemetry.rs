//! Read-only offline access to private typed traces. Does not read model credentials.
use clap::{Args as ClapArgs, Subcommand};
use namzu_core::AgentError;
use namzu_session::{PrivateStore, default_state_dir};
use namzu_telemetry::TraceId;
use serde_json::{Value, json};
use std::path::PathBuf;
#[derive(ClapArgs)]
pub struct Args {
    #[arg(long)]
    state_dir: Option<PathBuf>,
    #[command(subcommand)]
    action: Action,
}
#[derive(Subcommand)]
enum Action {
    List,
    Show { id: String },
}
pub fn run(args: Args) -> Result<Value, AgentError> {
    let store = PrivateStore::open(args.state_dir.map_or_else(default_state_dir, Ok)?)?;
    match args.action {
        Action::Show { id } => Ok(
            json!({"format_version":1,"status":"ok","trace":namzu_telemetry::read(&store,&TraceId::try_from(id)?)?,"offline":true}),
        ),
        Action::List => {
            // Missing telemetry is a normal disabled state; inspect existing names first.
            if !store.names()?.iter().any(|n| n == "telemetry") {
                return Ok(json!({"format_version":1,"status":"ok","traces":[],"offline":true}));
            }
            let child = store.open_child("telemetry")?;
            let mut traces = vec![];
            for name in child.names()? {
                if let Some(id) = name
                    .strip_suffix(".json")
                    .and_then(|s| TraceId::try_from(s.to_owned()).ok())
                {
                    if traces.len() >= 4096 {
                        return Err(AgentError::State(
                            "telemetry listing exceeds 4096; inspect an explicit trace ID",
                        ));
                    }
                    let t = namzu_telemetry::read(&store, &id)?;
                    traces.push(json!({"trace_id":id,"source":t.source,"started_unix_ms":t.started_unix_ms,"outcome":t.outcome,"elapsed_ms":t.elapsed_ms,"dropped_records":t.metrics.dropped_records}));
                }
            }
            Ok(json!({"format_version":1,"status":"ok","traces":traces,"offline":true}))
        }
    }
}
