//! Connect and ping an explicitly selected local MCP server. No model/account.
#[cfg(unix)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use namzu_core::{Cancellation, TurnContext};
    use namzu_mcp::{
        Limits,
        stdio::{Client, Spec},
        types::Implementation,
    };
    use serde_json::json;
    let mut args = std::env::args().skip(1);
    let program = args
        .next()
        .ok_or("usage: stdio /absolute/server [literal arguments...]")?;
    let mut spec = Spec::new(program, std::env::current_dir()?);
    spec.args = args.collect();
    let context = TurnContext::new(std::time::Duration::from_secs(300), Cancellation::default())?;
    let mut client = Client::connect(
        spec,
        Implementation::new("namzu-rs", "0.1.0"),
        Limits::default(),
        &context,
    )?;
    let server = client.server().unwrap().server_info.name.clone();
    let result = client.call("ping", None, &context)?;
    if result.is_err() {
        return Err("server returned RPC error".into());
    }
    let close = client.close();
    println!(
        "{}",
        json!({"transport":"stdio","server":server,"ping_ok":true,"exit_code":close.exit_code,"abandoned":close.abandoned.len(),"real_subprocess":true,"model_calls":0})
    );
    Ok(())
}
#[cfg(not(unix))]
fn main() {
    eprintln!("stdio process supervision currently requires Unix");
    std::process::exit(1);
}
