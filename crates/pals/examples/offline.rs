//! Runnable SDK example: no account, network or model quota.
use namzu_core::{
    Cancellation, Completion, Limits, NoopObserver, ScriptedProvider, ToolCall, TurnContext,
};
use namzu_pals::{Delegation, Job, PalCreate, Registry, TaskSpec, TaskStatus, Team};
use namzu_tools::{TerminalTools, ToolPolicy};
use serde_json::json;

fn answer() -> ScriptedProvider {
    ScriptedProvider(
        vec![Completion {
            text: "12".into(),
            tool_calls: vec![],
        }]
        .into(),
    )
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let mut tools = TerminalTools::new(root.path(), ToolPolicy::default())?;
    let context = TurnContext::new(Limits::default().timeout, Cancellation::default())?;
    // These are separate allocations from the demo host, not a running parent's budget.
    let delegation = Delegation::new("demo", &context, Limits::default(), &tools)?;
    let child = delegation.prepare(TaskSpec::new("child", "Return 12"))?;
    assert_eq!(
        child
            .run(&mut answer(), &mut tools, &mut NoopObserver)
            .result?
            .output,
        "12"
    );

    let team = Team::new("demo", &context, Limits::default(), &tools, 2)?;
    let mut jobs = vec![];
    for id in ["one", "two"] {
        jobs.push(Job::new(
            team.prepare(TaskSpec::new(id, "Return 12"))?,
            answer(),
            TerminalTools::new(root.path(), ToolPolicy::default())?,
            NoopObserver,
        ));
    }
    let results = team.run(jobs)?;
    assert_eq!(results.len(), 2);
    for result in results {
        assert_eq!(result.result?.output, "12");
    }

    let registry = Registry::open(root.path().join("state"), root.path().join("workspaces"))?;
    let pal = registry.create(PalCreate {
        owner: "demo".into(),
        name: "Reader".into(),
        purpose: "Read a fixture".into(),
        route: "scripted-fixture".into(),
        tools: None,
    })?;
    std::fs::write(pal.workspace.join("answer.txt"), "12\n")?;
    registry.queue_task("demo", &pal.id, "read", "Read answer.txt and return 12")?;
    let mut provider = ScriptedProvider(
        vec![
            Completion {
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "read".into(),
                    name: "read_file".into(),
                    arguments: json!({"path":"answer.txt"}),
                }],
            },
            Completion {
                text: "12".into(),
                tool_calls: vec![],
            },
        ]
        .into(),
    );
    let turn = registry.start_task("demo", &pal.id, "read")?.execute(
        &mut provider,
        &mut TerminalTools::new(&pal.workspace, ToolPolicy::default())?,
        &context,
        Limits::default(),
        &mut NoopObserver,
    )?;
    assert_eq!(turn.output, "12");
    assert_eq!(turn.stats.tool_calls, 1);
    drop(registry);
    let restored = Registry::open(root.path().join("state"), root.path().join("workspaces"))?;
    assert_eq!(
        restored.tasks("demo", &pal.id)?[0].status,
        TaskStatus::Complete
    );
    println!(
        "{}",
        json!({"offline":true,"child":"passed","parallel_team":"passed",
        "persistent_pal_read":"passed","reopened_session":"passed"})
    );
    Ok(())
}
