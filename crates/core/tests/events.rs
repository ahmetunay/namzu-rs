use namzu_core::*;
use serde_json::json;
use std::{cell::RefCell, rc::Rc};

#[derive(Default)]
struct Observed {
    events: Vec<Event>,
    order: Rc<RefCell<Vec<&'static str>>>,
    reject_tool_start: bool,
}
impl Observer for Observed {
    fn checkpoint(
        &mut self,
        _: &[Message],
        _: &Stats,
        pending: Option<&ToolCall>,
    ) -> Result<(), AgentError> {
        self.order.borrow_mut().push(if pending.is_some() {
            "pending"
        } else {
            "saved"
        });
        Ok(())
    }
    fn event(&mut self, event: Event) -> Result<(), AgentError> {
        if matches!(event, Event::ToolStarted { .. }) {
            self.order.borrow_mut().push("tool_started");
            if self.reject_tool_start {
                return Err(AgentError::Cancelled);
            }
        }
        if matches!(event, Event::ToolFinished { .. }) {
            self.order.borrow_mut().push("tool_finished");
        }
        self.events.push(event);
        Ok(())
    }
}
struct Effect(Rc<RefCell<Vec<&'static str>>>);
impl Tools for Effect {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "effect".into(),
            description: "fixture".into(),
            parameters: json!({"type":"object"}),
        }]
    }
    fn call(&mut self, _: &ToolCall, _: &TurnContext) -> Result<ToolResult, AgentError> {
        self.0.borrow_mut().push("effect");
        Ok(ToolResult::ok("result"))
    }
}
fn call() -> ToolCall {
    ToolCall {
        id: "id".into(),
        name: "effect".into(),
        arguments: json!({}),
    }
}
fn run(provider: &mut impl Provider, observer: &mut Observed) -> Result<Turn, AgentError> {
    run_history(
        provider,
        &mut Effect(observer.order.clone()),
        vec![Message::User {
            content: "test".into(),
        }],
        Limits::default(),
        Cancellation::default(),
        observer,
    )
}

#[test]
fn presentation_events_preserve_durable_pre_effect_order_and_buffered_provider_support() {
    let mut observer = Observed::default();
    let mut provider = ScriptedProvider(
        vec![
            Completion {
                text: "checking".into(),
                tool_calls: vec![call()],
            },
            Completion {
                text: "done".into(),
                tool_calls: vec![],
            },
        ]
        .into(),
    );
    let result = run(&mut provider, &mut observer).unwrap();
    assert_eq!(result.output, "done");
    assert_eq!(
        &*observer.order.borrow(),
        &[
            "saved",
            "pending",
            "tool_started",
            "effect",
            "saved",
            "tool_finished",
            "saved"
        ]
    );
    assert!(matches!(observer.events.first(), Some(Event::TurnStarted)));
    assert!(matches!(
        observer.events.last(),
        Some(Event::TurnCompleted { .. })
    ));
    assert_eq!(
        observer
            .events
            .iter()
            .filter(|e| matches!(e, Event::AssistantDelta { .. }))
            .count(),
        2
    );
}

#[test]
fn disconnected_presentation_stops_before_tool_effect_and_records_failure() {
    let mut observer = Observed {
        reject_tool_start: true,
        ..Observed::default()
    };
    let mut provider = ScriptedProvider(
        vec![Completion {
            text: "".into(),
            tool_calls: vec![call()],
        }]
        .into(),
    );
    assert_eq!(
        run(&mut provider, &mut observer).unwrap_err(),
        AgentError::Cancelled
    );
    assert!(!observer.order.borrow().contains(&"effect"));
    assert!(observer.order.borrow().contains(&"pending"));
    assert!(matches!(
        observer.events.last(),
        Some(Event::TurnFailed { .. })
    ));
}

#[test]
fn provisional_stream_failure_or_invalid_batch_never_commits_an_assistant_or_tool_effect() {
    struct Stream {
        broken: bool,
    }
    impl Provider for Stream {
        fn complete(
            &mut self,
            _: &[Message],
            _: &[ToolDefinition],
            _: &TurnContext,
        ) -> Result<Completion, AgentError> {
            panic!("stream method must be used")
        }
        fn complete_stream(
            &mut self,
            _: &[Message],
            _: &[ToolDefinition],
            _: &TurnContext,
            sink: &mut dyn EventSink,
        ) -> Result<Completion, AgentError> {
            sink.emit(Event::AssistantDelta {
                text: "provisional".into(),
            })?;
            if self.broken {
                return Err(AgentError::Protocol("truncated"));
            }
            Ok(Completion {
                text: "invalid batch".into(),
                tool_calls: vec![call(), call()],
            })
        }
    }
    for broken in [true, false] {
        let mut observer = Observed::default();
        assert!(run(&mut Stream { broken }, &mut observer).is_err());
        assert!(
            observer
                .events
                .iter()
                .any(|e| matches!(e, Event::AssistantDelta { .. }))
        );
        assert!(!observer.events.iter().any(|e| matches!(
            e,
            Event::AssistantMessage { .. }
                | Event::ToolStarted { .. }
                | Event::TurnCompleted { .. }
        )));
        assert!(!observer.order.borrow().contains(&"effect"));
    }
}

#[test]
fn denied_call_is_error_data_without_effect_and_cannot_approve_unknown_tools() {
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
    let effects = Rc::new(RefCell::new(vec![]));
    let mut unknown = call();
    unknown.name = "unavailable".into();
    unknown.id = "other".into();
    let mut provider = ScriptedProvider(
        vec![
            Completion {
                text: "".into(),
                tool_calls: vec![call(), unknown],
            },
            Completion {
                text: "done".into(),
                tool_calls: vec![],
            },
        ]
        .into(),
    );
    let mut observer = Deny(0);
    let turn = run_history(
        &mut provider,
        &mut Effect(effects.clone()),
        vec![Message::User {
            content: "test".into(),
        }],
        Limits::default(),
        Cancellation::default(),
        &mut observer,
    )
    .unwrap();
    assert!(effects.borrow().is_empty());
    assert_eq!(observer.0, 1);
    assert_eq!(
        turn.messages
            .iter()
            .filter(|m| matches!(m, Message::Tool { is_error: true, .. }))
            .count(),
        2
    );
}
