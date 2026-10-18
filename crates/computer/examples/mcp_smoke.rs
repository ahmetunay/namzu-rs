//! Explicit offline real-Docker acceptance; requires a prebuilt fixture image.
use namzu_computer::{
    ComputerManager, ComputerSpec, Docker, GuestPolicy, GuestTools, SharedGuestTools,
};
use namzu_core::{Cancellation, ToolCall, Tools, TurnContext};
use namzu_mcp::{
    agent::{AgentTools, Approvals, alias},
    config::{Document, Policy, Registry},
    discovery::Kind,
    manager::Manager,
};
use namzu_session::PrivateStore;
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let root = Path::new(&args[1]);
    let ctx = TurnContext::new(Duration::from_secs(300), Cancellation::default())?;
    let cm = ComputerManager::from_store(
        PrivateStore::open(root.join("computer"))?,
        "mcp-acceptance",
        "fixture",
        Docker::local("/usr/local/bin/docker")?,
    )?;
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let computer = cm.acquire(ComputerSpec::new(&args[2]), &ctx)?;
        let shared = SharedGuestTools::new(GuestTools::new(computer, GuestPolicy::default()));
        let r = Registry::open(&root.join("mcp"))?;
        r.replace(Document::parse(json!({"version":1,"default_profile":"default","profiles":{"default":{"servers":{"fixture":{"transport":{"type":"stdio","program":"/usr/local/bin/mcp-fixture","args":[],"cwd":"/workspace","env":{"DOCKER_HOST":"NAMZU_MCP_ENV_FIXTURE"}},"policy":{"require_approval":false}}}}}}).to_string().as_bytes())?)?;
        let manager = Manager::new(
            &r,
            None,
            None,
            Policy {
                require_approval: false,
                ..Policy::default()
            },
            &ctx,
        )?
        .with_guest(shared.launcher());
        let mut tools = AgentTools::new(shared.clone(), manager, Approvals::default());
        tools.load(&["fixture".into()], None, &Default::default(), &ctx, |p| {
            p.permit()
        })?;
        let result = tools.call(
            &ToolCall {
                id: "one".into(),
                name: alias("fixture", Kind::Tools, "write"),
                arguments: json!({}),
            },
            &ctx,
        )?;
        assert_eq!(
            serde_json::from_str::<Value>(&result.content)?["structuredContent"]["answer"],
            42
        );
        assert_eq!(
            serde_json::from_str::<Value>(&result.content)?["structuredContent"]["guest_env"],
            "tcp://unreachable.fixture.invalid:2375"
        );
        tools.close(&ctx)?;
        drop(tools);
        shared.finish_mcp(true, &ctx)?;
        let mut computer = shared.into_computer()?;
        assert!(computer.snapshot().pending.is_none());
        computer.start(&ctx)?;
        assert_eq!(computer.download("mcp-result", &ctx)?, b"42\n");
        // An interrupted scope keeps its durable marker. A freshly resumed
        // Computer must not inherit the original in-memory permission.
        let shared = SharedGuestTools::new(GuestTools::new(computer, GuestPolicy::default()));
        let manager = Manager::new(
            &r,
            None,
            None,
            Policy {
                require_approval: false,
                ..Policy::default()
            },
            &ctx,
        )?
        .with_guest(shared.launcher());
        let mut tools = AgentTools::new(shared.clone(), manager, Approvals::default());
        tools.load(&["fixture".into()], None, &Default::default(), &ctx, |p| {
            p.permit()
        })?;
        tools.close(&ctx)?;
        drop(tools);
        shared.finish_mcp(false, &ctx)?;
        let computer = shared.into_computer()?;
        assert_eq!(
            computer.snapshot().pending.as_ref().unwrap().kind,
            "mcp-active"
        );
        computer.detach(&ctx)?;
        let mut computer = cm.resume(&ctx)?;
        computer.start(&ctx)?;
        let shared = SharedGuestTools::new(GuestTools::new(computer, GuestPolicy::default()));
        let mut manager = Manager::new(
            &r,
            None,
            None,
            Policy {
                require_approval: false,
                ..Policy::default()
            },
            &ctx,
        )?
        .with_guest(shared.launcher());
        let permission = manager.prepare_connect("fixture")?.permit()?;
        assert!(manager.execute(permission, &ctx).is_err());
        manager.close(&ctx)?;
        drop(manager);
        shared.finish_mcp(true, &ctx)?;
        let mut computer = shared.into_computer()?;
        assert_eq!(
            computer.snapshot().pending.as_ref().unwrap().kind,
            "mcp-active"
        );
        computer.acknowledge_uncertain()?;
        computer.close(&ctx)?;
        Ok(())
    })();
    cm.recover_cleanup(&ctx)?;
    result?;
    println!("owned_guest_stdio_cleanup_and_uncertain_resume: passed");
    Ok(())
}
