use namzu_core::{
    AgentError, Cancellation, Completion, Limits, Message, NoopObserver, Observer,
    ScriptedProvider, Stats, ToolCall, Tools, TurnContext,
};
use namzu_pals::{Delegation, TaskSpec};
use namzu_tools::{TerminalTools, ToolPolicy};
use serde_json::json;
use std::time::Duration;

fn call(name: &str) -> ToolCall {
    ToolCall {
        id: "effect".into(),
        name: name.into(),
        arguments: json!({"path":"answer.txt","content":"changed"}),
    }
}
fn script(name: &str) -> ScriptedProvider {
    ScriptedProvider(
        vec![
            Completion {
                text: String::new(),
                tool_calls: vec![call(name)],
            },
            Completion {
                text: "done".into(),
                tool_calls: vec![],
            },
        ]
        .into(),
    )
}
fn setup() -> (tempfile::TempDir, TerminalTools, TurnContext) {
    let dir = tempfile::tempdir().unwrap();
    let tools = TerminalTools::new(
        dir.path(),
        ToolPolicy {
            allow_write: true,
            ..ToolPolicy::default()
        },
    )
    .unwrap();
    let ctx = TurnContext::new(Duration::from_secs(60), Cancellation::default()).unwrap();
    (dir, tools, ctx)
}

#[test]
fn defaults_read_only_even_when_factory_supplies_write_tools() {
    let (dir, mut tools, ctx) = setup();
    let scope = Delegation::new("parent", &ctx, Limits::default(), &tools).unwrap();
    let task = scope.prepare(TaskSpec::new("child", "do work")).unwrap();
    let result = task.run(&mut script("write_file"), &mut tools, &mut NoopObserver);
    let turn = result.result.unwrap();
    assert_eq!(result.owner, "parent");
    assert!(
        turn.messages
            .iter()
            .any(|m| matches!(m, Message::Tool { is_error: true, .. }))
    );
    assert!(!dir.path().join("answer.txt").exists());
}

#[test]
fn explicit_tools_intersect_parent_and_approval_is_not_bypassed() {
    let (dir, mut tools, ctx) = setup();
    struct Deny(usize);
    impl Observer for Deny {
        fn approve(&mut self, _: &ToolCall, _: &TurnContext) -> Result<bool, AgentError> {
            self.0 += 1;
            Ok(false)
        }
        fn checkpoint(
            &mut self,
            _: &[Message],
            _: &Stats,
            _: Option<&ToolCall>,
        ) -> Result<(), AgentError> {
            Ok(())
        }
    }
    let scope = Delegation::new("parent", &ctx, Limits::default(), &tools).unwrap();
    let mut spec = TaskSpec::new("child", "work");
    spec.tools = Some(["write_file".into(), "shell".into()].into());
    let mut deny = Deny(0);
    scope
        .prepare(spec)
        .unwrap()
        .run(&mut script("write_file"), &mut tools, &mut deny)
        .result
        .unwrap();
    assert_eq!(deny.0, 1);
    assert!(!dir.path().join("answer.txt").exists());
    let mut spec = TaskSpec::new("other", "work");
    spec.tools = Some(["shell".into()].into());
    let turn = scope
        .prepare(spec)
        .unwrap()
        .run(&mut script("shell"), &mut tools, &mut deny)
        .result
        .unwrap();
    assert_eq!(deny.0, 1); // unavailable calls don't even ask for approval
    assert!(
        turn.messages
            .iter()
            .any(|m| matches!(m, Message::Tool { is_error: true, .. }))
    );
}

#[test]
fn explicit_write_can_execute_with_host_grant() {
    let (dir, mut tools, ctx) = setup();
    let scope = Delegation::new("parent", &ctx, Limits::default(), &tools).unwrap();
    let mut spec = TaskSpec::new("child", "work");
    spec.tools = Some(["write_file".into()].into());
    scope
        .prepare(spec)
        .unwrap()
        .run(&mut script("write_file"), &mut tools, &mut NoopObserver)
        .result
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("answer.txt")).unwrap(),
        "changed"
    );
}

#[test]
fn reservations_are_atomic_cumulative_and_not_refunded() {
    let (_, tools, ctx) = setup();
    let scope = Delegation::new(
        "parent",
        &ctx,
        Limits {
            max_steps: 16,
            max_tool_calls: 16,
            max_text_bytes: 256 * 1024,
            ..Limits::default()
        },
        &tools,
    )
    .unwrap();
    let mut bad = TaskSpec::new("a", "work");
    bad.limits.max_tool_calls = 17;
    assert!(matches!(scope.prepare(bad), Err(AgentError::ToolLimit)));
    drop(scope.prepare(TaskSpec::new("a", "work")).unwrap()); // rejected reservation did not consume ID/steps
    assert!(matches!(
        scope.prepare(TaskSpec::new("b", "work")),
        Err(AgentError::StepLimit)
    ));
    assert!(scope.prepare(TaskSpec::new("a", "work")).is_err());
}

#[test]
fn byte_budget_is_shared_and_zero_tool_budget_is_valid() {
    let (_, tools, ctx) = setup();
    let scope = Delegation::new(
        "p",
        &ctx,
        Limits {
            max_tool_calls: 0,
            max_text_bytes: 100,
            ..Limits::default()
        },
        &tools,
    )
    .unwrap();
    let mut spec = TaskSpec::new("a", "x");
    spec.limits.max_tool_calls = 0;
    spec.limits.max_text_bytes = 101;
    assert!(scope.prepare(spec.clone()).is_err());
    spec.limits.max_text_bytes = 100;
    scope.prepare(spec).unwrap();
}

#[test]
fn parent_cancel_stops_children_and_child_cancel_leaves_sibling_live() {
    let (_, mut tools, ctx) = setup();
    let scope = Delegation::new("p", &ctx, Limits::default(), &tools).unwrap();
    let a = scope.prepare(TaskSpec::new("a", "x")).unwrap();
    let b = scope.prepare(TaskSpec::new("b", "x")).unwrap();
    a.cancellation().cancel();
    let mut provider = ScriptedProvider(
        vec![Completion {
            text: "yes".into(),
            tool_calls: vec![],
        }]
        .into(),
    );
    assert_eq!(
        a.run(&mut provider, &mut tools, &mut NoopObserver)
            .result
            .unwrap_err(),
        AgentError::Cancelled
    );
    assert_eq!(provider.0.len(), 1);
    b.run(&mut provider, &mut tools, &mut NoopObserver)
        .result
        .unwrap();
    ctx.cancellation().cancel();
    assert!(matches!(
        scope.prepare(TaskSpec::new("c", "x")),
        Err(AgentError::Cancelled)
    ));
}

#[test]
fn child_context_rejects_zero_timeout_and_observes_later_parent_cancel() {
    let ctx = TurnContext::new(Duration::from_secs(30), Cancellation::default()).unwrap();
    let child = ctx.child(Duration::from_secs(300)).unwrap();
    child.cancellation().cancel();
    assert!(!ctx.cancellation().is_cancelled());
    assert!(ctx.child(Duration::ZERO).is_err());
    let sibling = ctx.child(Duration::from_secs(300)).unwrap();
    ctx.cancellation().cancel();
    assert_eq!(sibling.remaining(), Err(AgentError::Cancelled));
}

#[test]
fn fresh_child_history_exposes_only_child_prompt_and_admitted_tools() {
    let (_, mut tools, ctx) = setup();
    struct Inspect;
    impl namzu_core::Provider for Inspect {
        fn complete(
            &mut self,
            messages: &[Message],
            tools: &[namzu_core::ToolDefinition],
            _: &TurnContext,
        ) -> Result<Completion, AgentError> {
            assert_eq!(
                messages,
                &[Message::User {
                    content: "child-only".into()
                }]
            );
            assert_eq!(tools.len(), 1);
            assert_eq!(tools[0].name, "read_file");
            Ok(Completion {
                text: "ok".into(),
                tool_calls: vec![],
            })
        }
    }
    let scope = Delegation::new("p", &ctx, Limits::default(), &tools).unwrap();
    let mut spec = TaskSpec::new("a", "child-only");
    spec.tools = Some(["read_file".into()].into());
    scope
        .prepare(spec)
        .unwrap()
        .run(&mut Inspect, &mut tools, &mut NoopObserver)
        .result
        .unwrap();
    assert!(tools.definitions().iter().any(|d| d.name == "write_file"));
}
