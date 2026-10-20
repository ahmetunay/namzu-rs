use super::*;
use namzu_core::{
    Cancellation, Completion, Limits, NoopObserver, ScriptedProvider, ToolDefinition, ToolResult,
    Tools, run_history,
};
use serde_json::json;
use std::time::Duration;
fn ctx() -> TurnContext {
    TurnContext::new(Duration::from_secs(5), Cancellation::default()).unwrap()
}
fn call() -> ToolCall {
    ToolCall {
        id: "SECRET_CALL_ID".into(),
        name: "write_file".into(),
        arguments: json!({"path":"SECRET_PATH","content":"SECRET_ARGUMENT"}),
    }
}
#[test]
fn projection_cannot_copy_raw_content_ids_names_or_error_strings() {
    let mut r = Recorder::new(Source::Sdk, 64).unwrap();
    for e in [
        Event::TurnStarted,
        Event::ProviderStarted { step: 1 },
        Event::AssistantDelta {
            text: "SECRET_DELTA".into(),
        },
        Event::AssistantMessage {
            text: "SECRET_MESSAGE".into(),
            has_tool_calls: true,
        },
        Event::ToolStarted {
            id: "SECRET_ID".into(),
            name: "mcp_SECRET_NAME".into(),
        },
        Event::ToolFinished {
            id: "SECRET_ID".into(),
            name: "SECRET_NAME".into(),
            result: ToolResult::error("SECRET_RESULT"),
        },
        Event::TurnFailed {
            error: "SECRET_ERROR https://secret-host?api_key=SECRET_KEY".into(),
        },
    ] {
        r.observe_at(&e, 5);
    }
    r.finish_at(Some(&AgentError::Provider("SECRET_PROVIDER".into())), 10);
    let text = serde_json::to_string(r.snapshot()).unwrap();
    assert!(!text.contains("SECRET"));
    assert!(!text.contains("secret-host"));
    assert!(text.contains("mcp"));
    r.snapshot().validate().unwrap();
}
#[test]
fn deterministic_spans_use_ms_and_parent_root_and_optional_usage() {
    let mut r = Recorder::new(Source::Sdk, 64).unwrap();
    r.observe_at(&Event::ProviderStarted { step: 1 }, 10);
    r.observe_at(
        &Event::AssistantMessage {
            text: "ok".into(),
            has_tool_calls: true,
        },
        30,
    );
    r.observe_at(
        &Event::ToolStarted {
            id: "a".into(),
            name: "read_file".into(),
        },
        31,
    );
    r.observe_at(
        &Event::ToolFinished {
            id: "a".into(),
            name: "read_file".into(),
            result: ToolResult::ok("ok"),
        },
        41,
    );
    r.finish_at(None, 50);
    let t = r.snapshot();
    assert_eq!(
        t.metrics.provider_duration,
        Timing {
            samples: 1,
            sum_ms: 20,
            max_ms: 20
        }
    );
    assert_eq!(t.metrics.tool_duration.sum_ms, 10);
    assert_eq!(t.elapsed_ms, 50);
    assert_eq!(t.metrics.usage.responses, 0);
    assert!(t.metrics.usage.input_tokens.is_none());
    assert!(
        t.records
            .iter()
            .filter(|r| r.span != 1)
            .all(|r| r.parent == Some(1))
    );
    t.validate().unwrap();
}
#[test]
fn reported_metadata_uses_partial_coverage_not_zero_or_invented_cost() {
    let mut r = Recorder::new(Source::Sdk, 64).unwrap();
    for metadata in [
        ResponseMetadata {
            usage: Some(namzu_core::TokenUsage {
                input_tokens: Some(3),
                output_tokens: Some(2),
                total_tokens: Some(5),
                ..Default::default()
            }),
            cost: None,
        },
        ResponseMetadata::default(),
    ] {
        r.observe(&Event::ProviderStarted { step: 1 });
        r.observe(&Event::ProviderFinished { metadata });
    }
    r.finish(None);
    let m = &r.snapshot().metrics;
    assert_eq!(m.usage.responses, 2);
    assert_eq!(m.usage.input_tokens.as_ref().unwrap().sum, 3);
    assert_eq!(m.usage.input_tokens.as_ref().unwrap().samples, 1);
    assert!(m.usage.reported_cost_nano_usd.is_none());
}
#[test]
fn invalid_metadata_is_not_published_and_does_not_interrupt_agent() {
    let mut r = Recorder::new(Source::Sdk, 64).unwrap();
    r.observe(&Event::ProviderStarted { step: 1 });
    r.observe(&Event::ProviderFinished {
        metadata: ResponseMetadata {
            usage: Some(namzu_core::TokenUsage {
                input_tokens: Some(1),
                cached_input_tokens: Some(2),
                ..Default::default()
            }),
            cost: None,
        },
    });
    r.finish(None);
    assert_eq!(r.snapshot().metrics.invalid_events, 1);
    assert_eq!(r.snapshot().metrics.usage.responses, 0);
    r.snapshot().validate().unwrap();
}
#[test]
fn bounded_records_keep_root_terminal_and_complete_numeric_counters() {
    let mut r = Recorder::new(Source::Sdk, 2).unwrap();
    for _ in 0..100 {
        r.observe(&Event::ProviderStarted { step: 1 });
        r.observe(&Event::AssistantMessage {
            text: "ok".into(),
            has_tool_calls: false,
        });
    }
    r.finish(None);
    assert_eq!(r.snapshot().records.len(), 2);
    assert_eq!(r.snapshot().metrics.provider_completed, 100);
    assert_eq!(r.snapshot().metrics.dropped_records, 200);
    r.snapshot().validate().unwrap();
}
#[test]
fn delta_flood_is_coalesced_not_retained_and_terminal_is_idempotent() {
    let mut r = Recorder::new(Source::Sdk, 10).unwrap();
    for _ in 0..10000 {
        r.observe_at(&Event::AssistantDelta { text: "ğ".into() }, 2);
    }
    r.finish_at(None, 5);
    let old = r.snapshot().clone();
    r.finish_at(Some(&AgentError::Cancelled), 8);
    r.observe(&Event::TurnStarted);
    r.approval(true);
    assert_eq!(r.snapshot(), &old);
    assert_eq!(old.metrics.delta_utf8_bytes, 20000);
    assert_eq!(old.records.len(), 2);
}
#[test]
fn regressing_clock_is_clamped_and_bad_event_order_is_counted() {
    let mut r = Recorder::new(Source::Sdk, 20).unwrap();
    r.observe_at(
        &Event::ProviderFinished {
            metadata: Default::default(),
        },
        10,
    );
    r.observe_at(&Event::ProviderStarted { step: 1 }, 1);
    r.observe_at(&Event::ProviderStarted { step: 2 }, 11);
    r.finish_at(Some(&AgentError::Timeout), 12);
    assert_eq!(r.snapshot().metrics.invalid_events, 3);
    r.snapshot().validate().unwrap();
}
#[test]
fn unfinished_provider_is_closed_with_typed_failure() {
    let mut r = Recorder::new(Source::Sdk, 20).unwrap();
    r.observe_at(&Event::ProviderStarted { step: 1 }, 5);
    r.finish_at(Some(&AgentError::Cancelled), 10);
    assert_eq!(
        r.snapshot().outcome,
        Outcome::Failed {
            failure: Failure::Cancelled
        }
    );
    assert!(r.snapshot().records.iter().any(|r| r.span == 2
        && matches!(
            r.event,
            TraceEvent::Ended {
                outcome: Outcome::Failed {
                    failure: Failure::Cancelled
                },
                ..
            }
        )));
    assert_eq!(r.snapshot().metrics.provider_completed, 0);
}
#[test]
fn arbitrary_failure_payloads_never_serialize() {
    for error in [
        AgentError::Provider("SECRET_TOKEN".into()),
        AgentError::Configuration("SECRET_CONFIG"),
        AgentError::State("SECRET_PATH"),
        AgentError::Protocol("SECRET_BODY"),
        AgentError::StepLimit,
        AgentError::ToolLimit,
        AgentError::Timeout,
        AgentError::Remote(namzu_core::ProviderFailure::http(429, Some(7))),
    ] {
        let text = serde_json::to_string(&Failure::of(&error)).unwrap();
        assert!(!text.contains("SECRET"));
    }
}
#[test]
fn private_pending_snapshot_survives_drop_and_final_snapshot_reads() {
    let dir = tempfile::tempdir().unwrap();
    let store = PrivateStore::open(dir.path()).unwrap();
    let id = {
        let trace = LocalTrace::start(&store, Source::Exec).unwrap();
        trace.recorder.snapshot().trace_id.clone()
    };
    assert_eq!(read(&store, &id).unwrap().outcome, Outcome::Running);
    let mut trace = LocalTrace::start(&store, Source::Pal).unwrap();
    trace.finish(None).unwrap();
    assert_eq!(
        read(&store, &trace.recorder.snapshot().trace_id)
            .unwrap()
            .outcome,
        Outcome::Ok
    );
    assert_ne!(id, trace.recorder.snapshot().trace_id);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(dir.path().join("telemetry"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(
                dir.path()
                    .join("telemetry")
                    .join(format!("{}.json", id.as_str()))
            )
            .unwrap()
            .permissions()
            .mode()
                & 0o777,
            0o600
        );
    }
}
#[test]
fn id_paths_versions_unknown_fields_and_inconsistent_end_are_rejected() {
    for id in ["../credentials", "t-abc", "s-1", "t-SECRET"] {
        assert!(TraceId::try_from(id.to_owned()).is_err());
    }
    let mut r = Recorder::new(Source::Sdk, 10).unwrap();
    r.finish(None);
    let mut v = serde_json::to_value(r.snapshot()).unwrap();
    v["prompt"] = json!("SECRET");
    assert!(serde_json::from_value::<Trace>(v).is_err());
    let mut t = r.snapshot().clone();
    t.schema_version = 2;
    assert!(t.validate().is_err());
    t.schema_version = 1;
    t.records.pop();
    assert!(t.validate().is_err());
}
#[test]
fn private_reader_refuses_symlink_and_nonprivate_file() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let store = PrivateStore::open(dir.path()).unwrap();
        let trace = LocalTrace::start(&store, Source::Sdk).unwrap();
        let id = &trace.recorder.snapshot().trace_id;
        let path = dir
            .path()
            .join("telemetry")
            .join(format!("{}.json", id.as_str()));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read(&store, id).is_err());
        std::fs::remove_file(&path).unwrap();
        symlink(dir.path().join("credentials"), path).unwrap();
        assert!(read(&store, id).is_err());
    }
}
struct Host {
    deny: bool,
    fail_event: bool,
    fail_checkpoint: bool,
}
impl Observer for Host {
    fn supports_approval(&self) -> bool {
        true
    }
    fn approve(&mut self, _: &ToolCall, c: &TurnContext) -> Result<bool, AgentError> {
        c.remaining()?;
        Ok(!self.deny)
    }
    fn checkpoint(
        &mut self,
        _: &[Message],
        _: &Stats,
        _: Option<&ToolCall>,
    ) -> Result<(), AgentError> {
        if self.fail_checkpoint {
            Err(AgentError::State("SECRET_JOURNAL"))
        } else {
            Ok(())
        }
    }
    fn event(&mut self, e: Event) -> Result<(), AgentError> {
        if self.fail_event && matches!(e, Event::ToolStarted { .. }) {
            Err(AgentError::Cancelled)
        } else {
            Ok(())
        }
    }
}
struct Effect(usize);
impl Tools for Effect {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "write_file".into(),
            description: "fixture".into(),
            parameters: json!({"type":"object"}),
        }]
    }
    fn call(&mut self, _: &ToolCall, _: &TurnContext) -> Result<ToolResult, AgentError> {
        self.0 += 1;
        Ok(ToolResult::ok("SECRET_RESULT"))
    }
}
fn turns(host: &mut Host) -> (Result<Turn, AgentError>, Recorder, usize) {
    let mut r = Recorder::new(Source::Sdk, 64).unwrap();
    let mut p = ScriptedProvider(
        [
            Completion {
                text: String::new(),
                tool_calls: vec![call()],
            },
            Completion {
                text: "SECRET_ANSWER".into(),
                tool_calls: vec![],
            },
        ]
        .into(),
    );
    let mut tools = Effect(0);
    let result = run_history(
        &mut p,
        &mut tools,
        vec![Message::User {
            content: "SECRET_PROMPT".into(),
        }],
        Limits::default(),
        Cancellation::default(),
        &mut Observed {
            inner: host,
            recorder: Some(&mut r),
        },
    );
    r.finish(result.as_ref().err());
    (result, r, tools.0)
}
#[test]
fn observed_preserves_denial_without_tool_effect_and_records_error_result() {
    let (result, r, effects) = turns(&mut Host {
        deny: true,
        fail_event: false,
        fail_checkpoint: false,
    });
    assert!(result.is_ok());
    assert_eq!(effects, 0);
    assert_eq!(r.snapshot().metrics.approvals_denied, 1);
    assert_eq!(r.snapshot().metrics.tool_errors, 1);
    assert_eq!(r.snapshot().metrics.provider_completed, 2);
    assert!(
        !serde_json::to_string(r.snapshot())
            .unwrap()
            .contains("SECRET")
    );
}
#[test]
fn observed_preserves_presentation_and_checkpoint_failure_before_effects() {
    for (event, checkpoint) in [(true, false), (false, true)] {
        let (result, r, effects) = turns(&mut Host {
            deny: false,
            fail_event: event,
            fail_checkpoint: checkpoint,
        });
        assert!(result.is_err());
        assert_eq!(effects, 0);
        r.snapshot().validate().unwrap();
    }
}
#[test]
fn observed_allows_one_effect_without_changing_turn_result() {
    let (result, r, effects) = turns(&mut Host {
        deny: false,
        fail_event: false,
        fail_checkpoint: false,
    });
    assert_eq!(result.unwrap().output, "SECRET_ANSWER");
    assert_eq!(effects, 1);
    assert_eq!(r.snapshot().metrics.approvals_allowed, 1);
}
#[test]
fn tap_forwards_cancelled_approval_and_sink_errors() {
    struct Sink;
    impl EventSink for Sink {
        fn supports_approval(&self) -> bool {
            true
        }
        fn approve(&mut self, _: &ToolCall, c: &TurnContext) -> Result<bool, AgentError> {
            c.remaining()?;
            Ok(false)
        }
        fn emit(&mut self, _: Event) -> Result<(), AgentError> {
            Err(AgentError::Cancelled)
        }
    }
    let mut r = Recorder::new(Source::Sdk, 10).unwrap();
    let mut s = Sink;
    let mut tap = Tap {
        inner: &mut s,
        recorder: Some(&mut r),
    };
    assert!(tap.supports_approval());
    assert!(!tap.approve(&call(), &ctx()).unwrap());
    let cancel = Cancellation::default();
    cancel.cancel();
    assert_eq!(
        tap.approve(
            &call(),
            &TurnContext::new(Duration::from_secs(1), cancel).unwrap()
        )
        .unwrap_err(),
        AgentError::Cancelled
    );
    assert_eq!(
        tap.emit(Event::TurnStarted).unwrap_err(),
        AgentError::Cancelled
    );
}
#[test]
fn publication_failure_preserves_original_error_and_does_not_claim_success() {
    let root = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let store = PrivateStore::open(root.path()).unwrap();
    let mut trace = LocalTrace::start(&store, Source::Sdk).unwrap();
    let id = trace.recorder.snapshot().trace_id.clone();
    trace.store = PrivateStore::open(other.path()).unwrap();
    std::fs::create_dir(other.path().join(format!("{}.json", id.as_str()))).unwrap();
    let mut trace = Some(trace);
    assert_eq!(
        finish_result(&mut trace, Err(AgentError::Cancelled)).unwrap_err(),
        AgentError::Cancelled
    );
    let turn = Turn {
        output: "SECRET".into(),
        stats: Stats {
            provider_calls: 0,
            tool_calls: 0,
            usage: Default::default(),
        },
        messages: vec![],
    };
    assert!(matches!(
        finish_result(&mut trace, Ok(turn)),
        Err(AgentError::State(_))
    ));
    assert_eq!(read(&store, &id).unwrap().outcome, Outcome::Running);
}
#[test]
fn recorder_limit_rejects_unbounded_configuration() {
    assert!(Recorder::new(Source::Sdk, 1).is_err());
    assert!(Recorder::new(Source::Sdk, MAX_RECORDS + 1).is_err());
}
#[test]
fn disabled_observer_is_transparent_and_does_not_require_storage() {
    let mut host = NoopObserver;
    let mut o = Observed {
        inner: &mut host,
        recorder: None,
    };
    assert!(!o.supports_approval());
    o.checkpoint(
        &[],
        &Stats {
            provider_calls: 0,
            tool_calls: 0,
            usage: Default::default(),
        },
        None,
    )
    .unwrap();
}

#[test]
fn tap_forwards_proposal_failure_without_copying_arguments_into_trace() {
    struct Sink;
    impl EventSink for Sink {
        fn tool_proposed(&mut self, c: &ToolCall) -> Result<(), AgentError> {
            assert_eq!(c.arguments["path"], "SECRET_PATH");
            Err(AgentError::Cancelled)
        }
        fn emit(&mut self, _: Event) -> Result<(), AgentError> {
            Ok(())
        }
    }
    let mut recorder = Recorder::new(Source::Agui, 10).unwrap();
    let mut sink = Sink;
    assert_eq!(
        Tap {
            inner: &mut sink,
            recorder: Some(&mut recorder)
        }
        .tool_proposed(&call())
        .unwrap_err(),
        AgentError::Cancelled
    );
    recorder.finish(Some(&AgentError::Cancelled));
    assert!(
        !serde_json::to_string(recorder.snapshot())
            .unwrap()
            .contains("SECRET")
    );
}
