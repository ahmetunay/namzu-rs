use namzu_core::*;
use serde_json::json;
use std::{collections::VecDeque, time::Duration};

#[derive(Default)]
struct Recorder {
    calls: Vec<ToolCall>,
    fail: bool,
}
impl Tools for Recorder {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "add".into(),
            description: "test fixture".into(),
            parameters: json!({"type":"object"}),
        }]
    }
    fn call(&mut self, call: &ToolCall, context: &TurnContext) -> Result<ToolResult, AgentError> {
        context.remaining()?;
        self.calls.push(call.clone());
        Ok(if self.fail {
            ToolResult::error("rejected")
        } else {
            ToolResult::ok("12")
        })
    }
}
fn call(id: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "add".into(),
        arguments: json!({"a":7,"b":5}),
    }
}
fn requests(calls: Vec<ToolCall>) -> Completion {
    Completion {
        text: "".into(),
        tool_calls: calls,
    }
}
fn final_answer() -> Completion {
    Completion {
        text: "12".into(),
        tool_calls: vec![],
    }
}
fn script(items: Vec<Completion>) -> ScriptedProvider {
    ScriptedProvider(VecDeque::from(items))
}
fn run(
    provider: &mut impl Provider,
    tools: &mut impl Tools,
    limits: Limits,
) -> Result<Turn, AgentError> {
    run_turn(provider, tools, "7+5", limits, Cancellation::default())
}

#[test]
fn tool_output_is_replayed_with_original_id() {
    struct CheckingProvider {
        calls: usize,
    }
    impl Provider for CheckingProvider {
        fn complete(
            &mut self,
            messages: &[Message],
            _: &[ToolDefinition],
            _: &TurnContext,
        ) -> Result<Completion, AgentError> {
            self.calls += 1;
            if self.calls == 1 {
                return Ok(requests(vec![call("sum")]));
            }
            assert!(
                matches!(&messages[2], Message::Tool {tool_call_id,content,is_error:false} if tool_call_id=="sum" && content=="12")
            );
            Ok(final_answer())
        }
    }
    let result = run(
        &mut CheckingProvider { calls: 0 },
        &mut Recorder::default(),
        Limits::default(),
    )
    .unwrap();
    assert_eq!(result.output, "12");
    assert_eq!(
        result.stats,
        Stats {
            usage: Default::default(),
            provider_calls: 2,
            tool_calls: 1
        }
    );
}

#[test]
fn whole_batch_is_budgeted_before_first_effect() {
    let mut tools = Recorder::default();
    let result = run(
        &mut script(vec![requests(vec![call("a"), call("b")])]),
        &mut tools,
        Limits {
            max_tool_calls: 1,
            ..Limits::default()
        },
    );
    assert_eq!(result.unwrap_err(), AgentError::ToolLimit);
    assert!(tools.calls.is_empty());
}

#[test]
fn duplicate_ids_reject_whole_batch_before_first_effect() {
    let mut tools = Recorder::default();
    let result = run(
        &mut script(vec![requests(vec![call("a"), call("a")])]),
        &mut tools,
        Limits::default(),
    );
    assert!(matches!(result, Err(AgentError::Protocol(_))));
    assert!(tools.calls.is_empty());
}

#[test]
fn reused_ids_across_steps_are_refused() {
    let mut tools = Recorder::default();
    let result = run(
        &mut script(vec![requests(vec![call("a")]), requests(vec![call("a")])]),
        &mut tools,
        Limits::default(),
    );
    assert!(matches!(result, Err(AgentError::Protocol(_))));
    assert_eq!(tools.calls.len(), 1);
}

#[test]
fn malformed_arguments_reject_batch_before_first_effect() {
    let mut malformed = call("bad");
    malformed.arguments = json!([1, 2]);
    let mut tools = Recorder::default();
    let result = run(
        &mut script(vec![requests(vec![call("a"), malformed])]),
        &mut tools,
        Limits::default(),
    );
    assert!(matches!(result, Err(AgentError::Protocol(_))));
    assert!(tools.calls.is_empty());
}

#[test]
fn cancellation_before_provider_does_no_work() {
    let cancellation = Cancellation::default();
    cancellation.cancel();
    let mut provider = script(vec![final_answer()]);
    let result = run_turn(
        &mut provider,
        &mut Recorder::default(),
        "prompt",
        Limits::default(),
        cancellation,
    );
    assert_eq!(result.unwrap_err(), AgentError::Cancelled);
    assert_eq!(provider.0.len(), 1);
}

#[test]
fn cancellation_while_provider_returns_prevents_tool_effect() {
    struct CancellingProvider(Cancellation);
    impl Provider for CancellingProvider {
        fn complete(
            &mut self,
            _: &[Message],
            _: &[ToolDefinition],
            _: &TurnContext,
        ) -> Result<Completion, AgentError> {
            self.0.cancel();
            Ok(requests(vec![call("a")]))
        }
    }
    let cancellation = Cancellation::default();
    let mut tools = Recorder::default();
    let result = run_turn(
        &mut CancellingProvider(cancellation.clone()),
        &mut tools,
        "prompt",
        Limits::default(),
        cancellation,
    );
    assert_eq!(result.unwrap_err(), AgentError::Cancelled);
    assert!(tools.calls.is_empty());
}

#[test]
fn cancellation_between_batch_calls_prevents_second_effect() {
    struct CancellingTools {
        cancellation: Cancellation,
        calls: usize,
    }
    impl Tools for CancellingTools {
        fn definitions(&self) -> Vec<ToolDefinition> {
            Recorder::default().definitions()
        }
        fn call(&mut self, _: &ToolCall, _: &TurnContext) -> Result<ToolResult, AgentError> {
            self.calls += 1;
            self.cancellation.cancel();
            Ok(ToolResult::ok("done"))
        }
    }
    let cancellation = Cancellation::default();
    let mut tools = CancellingTools {
        cancellation: cancellation.clone(),
        calls: 0,
    };
    let result = run_turn(
        &mut script(vec![requests(vec![call("a"), call("b")])]),
        &mut tools,
        "prompt",
        Limits::default(),
        cancellation,
    );
    assert_eq!(result.unwrap_err(), AgentError::Cancelled);
    assert_eq!(tools.calls, 1);
}

#[test]
fn step_exhaustion_is_failure() {
    let result = run(
        &mut script(vec![requests(vec![call("a")])]),
        &mut Recorder::default(),
        Limits {
            max_steps: 1,
            ..Limits::default()
        },
    );
    assert_eq!(result.unwrap_err(), AgentError::StepLimit);
}

#[test]
fn zero_timeout_is_configuration_error() {
    let result = run(
        &mut script(vec![]),
        &mut Recorder::default(),
        Limits {
            timeout: Duration::ZERO,
            ..Limits::default()
        },
    );
    assert!(matches!(result, Err(AgentError::Configuration(_))));
}

#[test]
fn byte_budget_applies_before_effects() {
    let mut tools = Recorder::default();
    let result = run(
        &mut script(vec![requests(vec![call("a")])]),
        &mut tools,
        Limits {
            max_text_bytes: 4,
            ..Limits::default()
        },
    );
    assert!(matches!(result, Err(AgentError::Protocol(_))));
    assert!(tools.calls.is_empty());
}

#[test]
fn unavailable_tool_becomes_error_data_without_dispatch() {
    let mut unknown = call("a");
    unknown.name = "shell".into();
    let mut tools = Recorder::default();
    let result = run(
        &mut script(vec![requests(vec![unknown]), final_answer()]),
        &mut tools,
        Limits::default(),
    )
    .unwrap();
    assert!(matches!(
        &result.messages[2],
        Message::Tool { is_error: true, .. }
    ));
    assert!(tools.calls.is_empty());
}

#[test]
fn tool_failure_is_replayed_and_can_be_recovered() {
    let mut tools = Recorder {
        fail: true,
        ..Recorder::default()
    };
    let result = run(
        &mut script(vec![requests(vec![call("a")]), final_answer()]),
        &mut tools,
        Limits::default(),
    )
    .unwrap();
    assert!(
        matches!(&result.messages[2],Message::Tool {is_error:true,content,..} if content=="rejected")
    );
    assert_eq!(result.output, "12");
}
