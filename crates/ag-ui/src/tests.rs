use super::*;
use crate::client::{ClientState, Decoder};
use namzu_core::Stats;
fn mapper() -> Mapper {
    Mapper::new("opaque/../thread", "run-α").unwrap()
}
fn call() -> ToolCall {
    ToolCall {
        id: "call-1".into(),
        name: "write_file".into(),
        arguments: json!({"path":"answer","content":"42\n"}),
    }
}
fn turn(output: &str) -> Result<Turn, AgentError> {
    Ok(Turn {
        output: output.into(),
        stats: Stats {
            provider_calls: 1,
            tool_calls: 0,
            usage: Default::default(),
        },
        messages: vec![],
    })
}
fn input() -> RunInput {
    serde_json::from_value(json!({"threadId":"../../thread","runId":"run","messages":[{"role":"user","id":"u","content":"hello"}],"tools":[],"context":[],"state":{},"forwardedProps":{}})).unwrap()
}
#[test]
fn request_admits_only_last_user_and_never_system_or_client_scope() {
    let mut i = input();
    assert_eq!(i.admit().unwrap(), ("u", "hello"));
    i.messages.insert(
        0,
        json!({"role":"assistant","id":"old","content":"display history only"}),
    );
    assert!(i.admit().is_ok());
    for role in ["system", "developer", "provider_state"] {
        i.messages[0]["role"] = json!(role);
        assert!(i.admit().is_err());
    }
    for key in ["cwd", "provider", "model", "apiKey", "resume"] {
        let mut v =
            serde_json::to_value(json!({"threadId":"t","runId":"r","messages":[]})).unwrap();
        v[key] = json!("SECRET");
        assert!(serde_json::from_value::<RunInput>(v).is_err());
    }
}
#[test]
fn frontend_tools_context_state_and_forwarded_props_are_not_permissions() {
    for n in 0..4 {
        let mut i = input();
        match n {
            0 => i.tools = vec![json!({"name":"shell"})],
            1 => i.context = vec![json!({"value":"grant"})],
            2 => i.state = json!({"allowWrite":true}),
            _ => i.forwarded_props = json!({"allowShell":true}),
        };
        assert!(i.admit().is_err());
    }
}
#[test]
fn opaque_ids_are_preserved_but_control_and_excessive_ids_rejected() {
    let mut m = mapper();
    let v = m.start().unwrap();
    assert!(matches!(&v[0],WireEvent::RunStarted{thread_id,..}if thread_id=="opaque/../thread"));
    for s in ["", "\n", "a\rb"] {
        assert!(Mapper::new(s, "run").is_err());
    }
    assert!(Mapper::new(&"a".repeat(257), "run").is_err());
}
#[test]
fn message_stream_is_not_duplicated_and_final_result_is_authoritative() {
    let mut m = mapper();
    let mut all = m.map(&Event::ProviderStarted { step: 1 }).unwrap();
    all.extend(
        m.map(&Event::AssistantDelta { text: "hel".into() })
            .unwrap(),
    );
    all.extend(
        m.map(&Event::AssistantMessage {
            text: "hello".into(),
            has_tool_calls: false,
        })
        .unwrap(),
    );
    all.extend(m.finish(&turn("settled"), None, None));
    let mut state = ClientState::default();
    for e in &all {
        state.accept(e).unwrap();
    }
    assert_eq!(state.output.as_deref(), Some("settled"));
    assert_eq!(
        all.iter()
            .filter_map(|e| if let WireEvent::TextContent { delta, .. } = e {
                Some(delta.as_str())
            } else {
                None
            })
            .collect::<String>(),
        "hello"
    );
    state.eof().unwrap();
}
#[test]
fn conflicting_provisional_text_gets_a_new_message_without_fake_append() {
    let mut m = mapper();
    m.start().unwrap();
    m.map(&Event::AssistantDelta {
        text: "draft".into(),
    })
    .unwrap();
    let events = m
        .map(&Event::AssistantMessage {
            text: "final".into(),
            has_tool_calls: false,
        })
        .unwrap();
    assert!(events.iter().any(
        |e| matches!(e,WireEvent::Custom{name,..}if name=="namzu.provisional_text_discarded")
    ));
    assert!(
        events
            .iter()
            .any(|e| matches!(e,WireEvent::TextContent{delta,..}if delta=="final"))
    );
}
#[test]
fn tool_args_are_announced_once_and_results_retain_identity_and_error_flag() {
    let mut m = mapper();
    let mut events = m.start().unwrap();
    events.extend(m.propose(&call()).unwrap());
    assert!(m.propose(&call()).unwrap().is_empty());
    events.extend(
        m.map(&Event::ToolStarted {
            id: call().id,
            name: call().name,
        })
        .unwrap(),
    );
    events.extend(
        m.map(&Event::ToolFinished {
            id: call().id,
            name: call().name,
            result: ToolResult::error("DENIED"),
        })
        .unwrap(),
    );
    events.extend(m.finish(&turn("ok"), None, None));
    let mut s = ClientState::default();
    for e in &events {
        s.accept(e).unwrap();
    }
    assert!(events.iter().any(|e|matches!(e,WireEvent::ToolResult{tool_call_id,metadata,..}if tool_call_id=="call-1"&&metadata["namzu"]["isError"]==true)));
}
#[test]
fn duplicate_id_with_changed_args_and_results_without_proposal_fail() {
    let mut m = mapper();
    m.start().unwrap();
    m.propose(&call()).unwrap();
    let mut c = call();
    c.arguments = json!({"path":"other"});
    assert!(m.propose(&c).is_err());
    assert!(
        m.map(&Event::ToolFinished {
            id: "absent".into(),
            name: "write".into(),
            result: ToolResult::ok("x")
        })
        .is_err()
    );
}
#[test]
fn terminal_success_waits_for_host_and_provisional_failure_text_is_not_echoed() {
    let mut m = mapper();
    m.start().unwrap();
    assert!(
        m.map(&Event::TurnFailed {
            error: "SECRET_API_KEY".into()
        })
        .unwrap()
        .is_empty()
    );
    let end = m.finish(&Err(AgentError::Provider("SECRET_BODY".into())), None, None);
    let v = serde_json::to_string(&end).unwrap();
    assert!(!v.contains("SECRET"));
    assert!(matches!(end.last(),Some(WireEvent::RunError{code,..})if code=="NAMZU_PROVIDER"));
    assert!(m.finish(&turn("wrong"), None, None).is_empty());
}
#[test]
fn incomplete_tool_cannot_finish_successfully() {
    let mut m = mapper();
    m.start().unwrap();
    m.propose(&call()).unwrap();
    assert!(matches!(
        m.finish(&turn("wrong"), None, None).last(),
        Some(WireEvent::RunError { .. })
    ))
}
#[test]
fn cancelled_and_timeout_runs_have_typed_terminal_codes() {
    for (error, code) in [
        (AgentError::Cancelled, "NAMZU_CANCELLED"),
        (AgentError::Timeout, "NAMZU_TIMEOUT"),
    ] {
        let mut m = mapper();
        assert!(
            matches!(m.finish(&Err(error),None,None).last(),Some(WireEvent::RunError{code:c,..})if c==code)
        );
    }
}
#[test]
fn private_state_and_images_are_not_serialized_on_public_events() {
    let mut m = mapper();
    m.start().unwrap();
    m.propose(&call()).unwrap();
    let image = namzu_core::ToolImage {
        width: 2,
        height: 2,
        png_base64: "SECRET_BASE64".into(),
    };
    let out = m
        .map(&Event::ToolFinished {
            id: call().id,
            name: call().name,
            result: ToolResult {
                content: "pixels".into(),
                is_error: false,
                image: Some(image),
            },
        })
        .unwrap();
    let text = serde_json::to_string(&out).unwrap();
    assert!(!text.contains("SECRET_BASE64"));
    assert!(text.contains("namzu.image_available"));
}
#[test]
fn byte_fragmented_unicode_crlf_sse_decodes_exactly() {
    let e = WireEvent::TextContent {
        message_id: "m".into(),
        delta: "Türkçe 🦀\nline".into(),
    };
    let bytes = sse(&e).unwrap();
    let mut d = Decoder::default();
    let mut events = vec![];
    for b in bytes {
        events.extend(d.feed(&[b]).unwrap());
    }
    d.eof().unwrap();
    assert_eq!(events, vec![e]);
    let mut d = Decoder::default();
    assert_eq!(d.feed(b": heartbeat\r\n\r\ndata: {").unwrap().len(), 0);
    assert!(d.eof().is_err());
}
#[test]
fn sse_and_client_reject_truncated_eof_and_fake_success() {
    let mut d = Decoder::default();
    d.feed(b"data: {\"type\":").unwrap();
    assert!(d.eof().is_err());
    let mut s = ClientState::default();
    assert!(s.eof().is_err());
    assert!(
        s.accept(&WireEvent::TextEnd {
            message_id: "orphan".into()
        })
        .is_err()
    );
}
#[test]
fn client_rejects_wrong_ids_duplicate_run_and_event_after_terminal() {
    let mut s = ClientState::default();
    let start = WireEvent::RunStarted {
        thread_id: "t".into(),
        run_id: "r".into(),
    };
    s.accept(&start).unwrap();
    assert!(s.accept(&start).is_err());
    assert!(
        s.accept(&WireEvent::RunFinished {
            thread_id: "wrong".into(),
            run_id: "r".into(),
            result: json!({"output":"x"})
        })
        .is_err()
    );
    s.accept(&WireEvent::RunError {
        message: "x".into(),
        code: "CANCELLED".into(),
    })
    .unwrap();
    assert!(s.accept(&start).is_err());
}
#[test]
fn oversized_args_frame_and_input_text_are_rejected_before_effects() {
    let mut m = mapper();
    m.start().unwrap();
    let mut c = call();
    c.arguments = json!({"huge":"a".repeat(65537)});
    assert!(m.propose(&c).is_err());
    assert!(
        sse(&WireEvent::Custom {
            name: "x".into(),
            value: json!("x".repeat(MAX_FRAME))
        })
        .is_err()
    );
    let mut i = input();
    i.messages[0]["content"] = json!("x".repeat(65537));
    assert!(i.admit().is_err());
    let mut m = mapper();
    let mut client = ClientState::default();
    for event in m.start().unwrap() {
        client.accept(&event).unwrap();
    }
    assert!(
        m.map(&Event::AssistantDelta {
            text: "x".repeat(MAX_FRAME)
        })
        .is_err()
    );
    for event in m.finish(&Err(AgentError::Protocol("frame budget")), None, None) {
        client.accept(&event).unwrap();
    }
    client.eof().unwrap();
}
#[test]
fn stream_budget_prevents_unbounded_delta_retention() {
    let mut m = mapper();
    m.start().unwrap();
    let e = Event::AssistantDelta {
        text: "x".repeat(512 * 1024),
    };
    let mut stopped = false;
    for _ in 0..64 {
        if m.map(&e).is_err() {
            stopped = true;
            break;
        }
    }
    assert!(stopped);
}
#[test]
fn tool_and_step_lifecycle_order_is_checked_by_client() {
    let mut s = ClientState::default();
    s.accept(&WireEvent::RunStarted {
        thread_id: "t".into(),
        run_id: "r".into(),
    })
    .unwrap();
    assert!(
        s.accept(&WireEvent::ToolResult {
            message_id: "x".into(),
            tool_call_id: "none".into(),
            role: "tool".into(),
            content: "x".into(),
            metadata: json!({})
        })
        .is_err()
    );
    assert!(
        s.accept(&WireEvent::StepFinished {
            step_name: "none".into()
        })
        .is_err()
    );
}
#[test]
fn approval_extension_is_one_bounded_public_payload_not_a_tool_result() {
    let mut m = mapper();
    m.start().unwrap();
    let e = m.approval("a", &call()).unwrap();
    assert!(
        matches!(&e[0],WireEvent::Custom{name,value}if name=="namzu.approval_required" && value["arguments"]["path"]=="answer")
    );
    assert!(matches!(
        m.finish(&turn("ok"), None, None).last(),
        Some(WireEvent::RunFinished { .. })
    ));
}
#[test]
fn aggregate_only_provider_and_empty_deltas_have_valid_message_lifecycle() {
    let mut m = mapper();
    let mut e = m.start().unwrap();
    e.extend(
        m.map(&Event::AssistantDelta {
            text: String::new(),
        })
        .unwrap(),
    );
    e.extend(
        m.map(&Event::AssistantMessage {
            text: "buffered".into(),
            has_tool_calls: false,
        })
        .unwrap(),
    );
    e.extend(m.finish(&turn("buffered"), None, None));
    let mut s = ClientState::default();
    for event in e {
        s.accept(&event).unwrap();
    }
    s.eof().unwrap();
}
