//! Explicit remote MCP ping, no model call. Bearer token optional via named env.
use namzu_core::{Cancellation, TurnContext};
use namzu_mcp::{
    http::{Client, Spec},
    types::Implementation,
};
use serde_json::json;
use std::time::Duration;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let endpoint = args
        .next()
        .ok_or("usage: http ENDPOINT [BEARER_ENV_NAME]")?;
    let mut spec = Spec::new(endpoint);
    if let Some(name) = args.next() {
        spec.bearer =
            Some(std::env::var(name).map_err(|_| "bearer environment variable unavailable")?);
    }
    if args.next().is_some() {
        return Err("too many arguments".into());
    }
    let ctx = TurnContext::new(Duration::from_secs(300), Cancellation::default())?;
    let mut client = Client::connect(spec, Implementation::new("namzu-rs", "0.1.0"), &ctx)?;
    let result = client.call("ping", None, &ctx);
    let cleanup = TurnContext::new(Duration::from_secs(5), Cancellation::default())?;
    let closed = client.close(&cleanup);
    result?;
    closed?;
    println!(
        "{}",
        json!({"transport":"streamable-http","ping":true,"closed":true,"model_calls":0})
    );
    Ok(())
}
