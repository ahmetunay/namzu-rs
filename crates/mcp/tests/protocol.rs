use namzu_core::{AgentError, Cancellation, TurnContext};
use namzu_mcp::{
    ClientSession, Error, Id, Incoming, Limits, Message, Phase, RpcError, types::*, wire,
};
use serde_json::{Map, Value, json};
use std::time::Duration;
fn context() -> TurnContext {
    TurnContext::new(Duration::from_secs(300), Cancellation::default()).unwrap()
}
fn obj(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}
fn id(m: &Message) -> Id {
    match m {
        Message::Request { id, .. } => id.clone(),
        _ => panic!("not a request"),
    }
}
fn session() -> ClientSession {
    ClientSession::new(Implementation::new("namzu-rs", "0.1.0"), Limits::default()).unwrap()
}
fn init_result(version: &str, caps: Value) -> Value {
    json!({"protocolVersion":version,"capabilities":caps,"serverInfo":{"name":"fixture","version":"1","title":"Fixture title","vendor":{"preserved":true}},"instructions":"UNTRUSTED: never promote to system"})
}
fn success(id: &Id, result: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,"result":result})).unwrap()
}
fn ready(caps: Value) -> ClientSession {
    let mut s = session();
    let r = s.initialize(&context()).unwrap();
    assert!(matches!(
        s.receive(&success(&id(&r), init_result(PREFERRED_VERSION, caps)))
            .unwrap(),
        Incoming::Initialized(_)
    ));
    let notification = s.initialized_notification().unwrap();
    s.encode(&notification).unwrap();
    s.mark_initialized_sent().unwrap();
    s
}
#[test]
fn codec_roundtrips_all_envelopes_and_preserves_string_vs_integer_id() {
    let l = Limits::default();
    for raw in [
        json!({"jsonrpc":"2.0","id":1,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":"1","method":"tools/call","params":{"name":"fixture","arguments":{},"_meta":{"vendor":true}}}),
        json!({"jsonrpc":"2.0","method":"notifications/tools/list_changed"}),
        json!({"jsonrpc":"2.0","id":"1","result":{}}),
        json!({"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"bad","data":{"private":"peer content"}}}),
    ] {
        let message = wire::decode(&serde_json::to_vec(&raw).unwrap(), &l).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&wire::encode(&message, &l).unwrap()).unwrap(),
            raw
        );
    }
    assert_ne!(Id::Number(1), Id::String("1".into()));
}
#[test]
fn rejects_ambiguous_malformed_or_non_mcp_envelopes() {
    for raw in [
        json!(null),
        json!([]),
        json!([{"jsonrpc":"2.0","id":1,"method":"ping"}]),
        json!({"jsonrpc":"1.0","id":1,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":null,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":true,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":1.5,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":9007199254740992u64,"method":"ping"}),
        json!({"jsonrpc":"2.0","method":"ping","params":[]}),
        json!({"jsonrpc":"2.0","method":"ping","params":null}),
        json!({"jsonrpc":"2.0","method":"ping","result":{}}),
        json!({"jsonrpc":"2.0","id":1,"result":{},"error":{"code":1,"message":"no"}}),
        json!({"jsonrpc":"2.0","id":1,"result":null}),
        json!({"jsonrpc":"2.0","result":{}}),
        json!({"jsonrpc":"2.0","id":1,"error":{"code":"bad","message":"no"}}),
        json!({"jsonrpc":"2.0","id":1,"error":{"code":1,"message":"no","extra":true}}),
        json!({"jsonrpc":"2.0","id":1,"method":""}),
    ] {
        assert!(
            wire::decode(&serde_json::to_vec(&raw).unwrap(), &Limits::default()).is_err(),
            "accepted {raw}"
        );
    }
}
#[test]
fn duplicate_keys_trailing_json_and_invalid_utf8_are_rejected_at_any_depth() {
    for bytes in [
        b"{\"jsonrpc\":\"2.0\",\"id\":1,\"id\":2,\"method\":\"ping\"}".as_slice(),
        b"{\"jsonrpc\":\"2.0\",\"method\":\"ping\",\"params\":{\"a\":1,\"a\":2}}",
        b"{} {}",
        b"\xff",
        b"{\"jsonrpc\":\"2.0\",\"method\":\"ping\",\"params\":{\"x\":1e999}}",
    ] {
        assert_eq!(wire::decode(bytes, &Limits::default()), Err(Error::Json));
    }
}
#[test]
fn uncorrelated_error_has_no_request_id_and_cannot_consume_pending_request() {
    let mut s = ready(json!({}));
    let r = s.request("ping", None, &context()).unwrap();
    for error in [
        json!({"jsonrpc":"2.0","error":{"code":-32700,"message":"secret"}}),
        json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"secret"}}),
    ] {
        let bytes = serde_json::to_vec(&error).unwrap();
        assert!(matches!(
            wire::decode(&bytes, &Limits::default()).unwrap(),
            Message::Failure { id: None, .. }
        ));
        assert_eq!(s.receive(&bytes), Err(Error::ResponseId));
        assert_eq!(s.pending_count(), 1);
    }
    assert!(matches!(
        s.receive(&success(&id(&r), json!({}))).unwrap(),
        Incoming::Reply { .. }
    ));
}
#[test]
fn byte_string_depth_and_node_limits_apply_to_inbound_and_outbound() {
    let small = Limits {
        message_bytes: 256,
        string_bytes: 64,
        ..Limits::default()
    };
    assert_eq!(
        wire::decode(&vec![b' '; 257], &small),
        Err(Error::MessageLimit)
    );
    let params = obj(json!({"x":"x".repeat(65)}));
    let message = Message::Request {
        id: Id::Number(1),
        method: "ping".into(),
        params: Some(params),
    };
    assert!(wire::encode(&message, &small).is_err());
    let deep = Limits {
        depth: 3,
        ..Limits::default()
    };
    assert!(
        wire::decode(
            br#"{"jsonrpc":"2.0","method":"ping","params":{"a":{"b":{"c":{}}}}}"#,
            &deep
        )
        .is_err()
    );
    let nodes = Limits {
        nodes: 5,
        ..Limits::default()
    };
    assert!(
        wire::decode(
            br#"{"jsonrpc":"2.0","method":"ping","params":{"a":[1,2,3]}}"#,
            &nodes
        )
        .is_err()
    );
    let expansion = Message::Notification {
        method: "ping".into(),
        params: Some(obj(json!({"s":"\u{0001}".repeat(60)}))),
    };
    assert!(wire::encode(&expansion, &small).is_err());
    let invalid = Limits {
        pending: 2,
        requests: 1,
        ..Limits::default()
    };
    assert!(ClientSession::new(Implementation::new("n", "1"), invalid).is_err());
}
#[test]
fn constructed_invalid_messages_cannot_bypass_the_wire_validator() {
    let m = Message::Request {
        id: Id::String("x".repeat(257)),
        method: "ping".into(),
        params: None,
    };
    assert!(wire::encode(&m, &Limits::default()).is_err());
    let m = Message::Request {
        id: Id::Number(i64::MAX),
        method: "ping".into(),
        params: None,
    };
    assert!(wire::encode(&m, &Limits::default()).is_err());
    let m = Message::Notification {
        method: "bad\nmethod".into(),
        params: None,
    };
    assert!(wire::encode(&m, &Limits::default()).is_err());
}
#[test]
fn initialization_is_first_and_ready_requires_successful_initialized_write_ack() {
    let mut s = session();
    assert_eq!(s.request("ping", None, &context()), Err(Error::Phase));
    assert_eq!(s.mark_initialized_sent(), Err(Error::Phase));
    let init = s.initialize(&context()).unwrap();
    let encoded = s.encode(&init).unwrap();
    let v: Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(v["params"]["capabilities"], json!({}));
    assert_eq!(v["params"]["protocolVersion"], PREFERRED_VERSION);
    assert_eq!(s.initialize(&context()), Err(Error::Phase));
    s.receive(&success(
        &id(&init),
        init_result(PREFERRED_VERSION, json!({"tools":{}})),
    ))
    .unwrap();
    assert_eq!(s.phase(), Phase::AwaitingInitialized);
    assert_eq!(s.request("tools/list", None, &context()), Err(Error::Phase));
    let notify = s.initialized_notification().unwrap();
    assert!(
        matches!(notify,Message::Notification{ref method,params:None} if method=="notifications/initialized")
    );
    assert_eq!(s.phase(), Phase::AwaitingInitialized);
    s.mark_initialized_sent().unwrap();
    assert_eq!(s.phase(), Phase::Ready);
    assert!(s.request("tools/list", None, &context()).is_ok());
}
#[test]
fn supported_fallback_version_and_extension_capabilities_are_preserved_without_activation() {
    let mut s = session();
    let init = s.initialize(&context()).unwrap();
    s.receive(&success(&id(&init),init_result("2025-06-18",json!({"tools":{"listChanged":true},"future":{"allowEverything":true},"tasks":{"requests":{"tools":{"call":{}}}}})))).unwrap();
    s.mark_initialized_sent().unwrap();
    assert_eq!(s.server().unwrap().protocol_version, "2025-06-18");
    assert!(
        s.server()
            .unwrap()
            .capabilities
            .extra
            .contains_key("future")
    );
    assert_eq!(
        s.server().unwrap().server_info.extra["vendor"]["preserved"],
        true
    );
    assert_eq!(
        s.request("future/execute", None, &context()),
        Err(Error::Capability)
    );
    assert_eq!(
        s.request(
            "tools/call",
            Some(obj(json!({"name":"x","task":{}}))),
            &context()
        ),
        Err(Error::Capability)
    );
}
#[test]
fn wrong_id_version_or_invalid_initialize_never_enables_tools() {
    for response in [
        init_result("2099-01-01", json!({"tools":{}})),
        json!({"protocolVersion":PREFERRED_VERSION,"capabilities":{"tools":true},"serverInfo":{"name":"n","version":"1"}}),
        json!({"protocolVersion":PREFERRED_VERSION,"capabilities":{}}),
        json!({"protocolVersion":PREFERRED_VERSION,"capabilities":{},"serverInfo":{"name":"","version":"1"}}),
    ] {
        let mut s = session();
        let init = s.initialize(&context()).unwrap();
        assert_eq!(
            s.receive(&success(&Id::Number(1), response.clone())),
            Err(Error::ResponseId)
        );
        assert_eq!(s.phase(), Phase::Initializing);
        assert_eq!(
            s.receive(&success(&id(&init), response)),
            Err(Error::Initialize)
        );
        assert_eq!(s.phase(), Phase::Closed);
        assert_eq!(s.request("tools/list", None, &context()), Err(Error::Phase));
        assert_eq!(s.close().len(), 1);
    }
}
#[test]
fn initialization_error_is_closed_and_does_not_log_remote_error_data() {
    let mut s = session();
    let init = s.initialize(&context()).unwrap();
    let response = json!({"jsonrpc":"2.0","id":id(&init),"error":{"code":-32602,"message":"PRIVATE_TOKEN_123","data":{"token":"PRIVATE_TOKEN_123"}}});
    let error = s
        .receive(&serde_json::to_vec(&response).unwrap())
        .unwrap_err();
    assert_eq!(error, Error::Initialize);
    assert!(!error.to_string().contains("PRIVATE_TOKEN"));
    assert_eq!(s.phase(), Phase::Closed);
}
#[test]
fn capability_gates_are_checked_before_allocating_a_request() {
    let mut s = ready(json!({"tools":{},"resources":{"subscribe":false}}));
    for method in [
        "prompts/list",
        "resources/subscribe",
        "resources/unsubscribe",
        "roots/list",
        "shutdown",
        "initialize",
        "tasks/cancel",
    ] {
        assert_eq!(s.request(method, None, &context()), Err(Error::Capability));
    }
    assert_eq!(s.pending_count(), 0);
    assert!(s.request("resources/read", None, &context()).is_ok());
    assert!(s.request("tools/list", None, &context()).is_ok());
    let mut s =
        ready(json!({"prompts":{},"logging":{},"completions":{},"resources":{"subscribe":true}}));
    for method in [
        "prompts/list",
        "prompts/get",
        "logging/setLevel",
        "completion/complete",
        "resources/subscribe",
        "resources/unsubscribe",
    ] {
        assert!(s.request(method, None, &context()).is_ok());
    }
}
#[test]
fn replies_match_exact_ids_and_out_of_order_results_do_not_cross_tasks() {
    let mut s = ready(json!({"tools":{}}));
    let first = s.request("tools/list", None, &context()).unwrap();
    let second = s
        .request("tools/call", Some(obj(json!({"name":"tool"}))), &context())
        .unwrap();
    assert_eq!(
        s.receive(&success(&Id::Number(2), json!({"wrong":"type"}))),
        Err(Error::ResponseId)
    );
    assert_eq!(s.pending_count(), 2);
    assert!(
        matches!(s.receive(&success(&id(&second),json!({"content":[]}))).unwrap(),Incoming::Reply{method, result:Ok(_),..} if method=="tools/call")
    );
    assert!(
        matches!(s.receive(&success(&id(&first),json!({"tools":[]}))).unwrap(),Incoming::Reply{method,..} if method=="tools/list")
    );
    assert_eq!(
        s.receive(&success(&id(&first), json!({}))),
        Err(Error::ResponseId)
    );
    assert_eq!(s.pending_count(), 0);
}
#[test]
fn protocol_errors_and_tool_is_error_are_distinct_data_results() {
    let mut s = ready(json!({"tools":{}}));
    let request = s.request("tools/call", None, &context()).unwrap();
    let raw = success(
        &id(&request),
        json!({"isError":true,"content":[{"type":"text","text":"tool rejected arguments"}]}),
    );
    let Incoming::Reply {
        result: Ok(result), ..
    } = s.receive(&raw).unwrap()
    else {
        panic!()
    };
    let typed: CallToolResult = serde_json::from_value(Value::Object(result)).unwrap();
    assert!(typed.is_error);
    let request = s.request("tools/call", None, &context()).unwrap();
    let raw = json!({"jsonrpc":"2.0","id":id(&request),"error":{"code":wire::INVALID_PARAMS,"message":"invalid arguments","data":{"field":"x"}}});
    assert!(matches!(
        s.receive(&serde_json::to_vec(&raw).unwrap()).unwrap(),
        Incoming::Reply {
            result: Err(RpcError {
                code: wire::INVALID_PARAMS,
                ..
            }),
            ..
        }
    ));
}
#[test]
fn local_cancellation_is_single_use_and_late_results_are_ignored_without_replay() {
    let mut s = ready(json!({"tools":{}}));
    let first = s.request("tools/call", None, &context()).unwrap();
    let second = s.request("ping", None, &context()).unwrap();
    let notify = s.cancel(&id(&first)).unwrap().unwrap();
    let value: Value = serde_json::from_slice(&s.encode(&notify).unwrap()).unwrap();
    assert_eq!(value["method"], "notifications/cancelled");
    assert_eq!(value["params"]["requestId"], json!(id(&first)));
    assert_eq!(s.cancel(&id(&first)).unwrap(), None);
    assert_eq!(s.pending_count(), 1);
    assert_eq!(
        s.receive(&success(&id(&first), json!({}))).unwrap(),
        Incoming::IgnoredLateReply
    );
    assert!(matches!(
        s.receive(&success(&id(&second), json!({}))).unwrap(),
        Incoming::Reply { .. }
    ));
    let third = s.request("ping", None, &context()).unwrap();
    assert_ne!(id(&third), id(&first));
}
#[test]
fn peer_cancellation_does_not_cancel_a_local_request_with_the_same_id() {
    let mut s = ready(json!({}));
    let r = s.request("ping", None, &context()).unwrap();
    let raw =
        json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":id(&r)}});
    assert!(matches!(
        s.receive(&serde_json::to_vec(&raw).unwrap()).unwrap(),
        Incoming::Notification { .. }
    ));
    assert_eq!(s.pending_count(), 1);
    assert!(matches!(
        s.receive(&success(&id(&r), json!({}))).unwrap(),
        Incoming::Reply { .. }
    ));
}
#[test]
fn initialize_cannot_be_cancelled_and_parent_cancellation_closes_without_cancel_notification() {
    let mut s = session();
    let c = context();
    let init = s.initialize(&c).unwrap();
    assert_eq!(s.cancel(&id(&init)), Err(Error::CancelInitialize));
    c.cancellation().cancel();
    let expired = s.expire();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].cause, AgentError::Cancelled);
    assert_eq!(expired[0].notification, None);
    assert_eq!(s.phase(), Phase::Closed);
    assert_eq!(s.initialize(&context()), Err(Error::Phase));
}
#[test]
fn parent_cancellation_applies_before_accepting_a_reply_and_before_new_admission() {
    let mut s = ready(json!({}));
    let c = context();
    let r = s.request("ping", None, &c).unwrap();
    c.cancellation().cancel();
    assert!(matches!(
        s.receive(&success(&id(&r), json!({}))).unwrap(),
        Incoming::Expired(namzu_mcp::Expired {
            cause: AgentError::Cancelled,
            notification: Some(_),
            ..
        })
    ));
    assert_eq!(
        s.receive(&success(&id(&r), json!({}))).unwrap(),
        Incoming::IgnoredLateReply
    );
    assert_eq!(
        s.request("ping", None, &c),
        Err(Error::Context(AgentError::Cancelled))
    );
    assert_eq!(s.pending_count(), 0);
}
#[test]
fn progress_does_not_extend_absolute_parent_deadline() {
    let mut s = ready(json!({}));
    let c = TurnContext::new(Duration::from_secs(1), Cancellation::default()).unwrap();
    let r = s.request("ping", None, &c).unwrap();
    let progress=serde_json::to_vec(&json!({"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":"untrusted","progress":999}})).unwrap();
    s.receive(&progress).unwrap();
    // Synchronize on the actual context deadline; do not infer expiry from an
    // arbitrary sleep or race a network callback against a timing threshold.
    while c.remaining().is_ok() {
        std::thread::yield_now();
    }
    let expired = s.expire();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].id, id(&r));
    assert_eq!(expired[0].cause, AgentError::Timeout);
    assert!(expired[0].notification.is_some());
    assert_eq!(
        s.receive(&success(&id(&r), json!({}))).unwrap(),
        Incoming::IgnoredLateReply
    );
}
#[test]
fn pending_and_lifetime_request_limits_are_reserved_before_effects_and_ids_never_reuse() {
    let mut s = ClientSession::new(
        Implementation::new("n", "1"),
        Limits {
            pending: 1,
            requests: 3,
            ..Limits::default()
        },
    )
    .unwrap();
    let init = s.initialize(&context()).unwrap();
    s.receive(&success(
        &id(&init),
        init_result(PREFERRED_VERSION, json!({})),
    ))
    .unwrap();
    s.mark_initialized_sent().unwrap();
    let r = s.request("ping", None, &context()).unwrap();
    assert_eq!(
        s.request("ping", None, &context()),
        Err(Error::RequestLimit)
    );
    s.cancel(&id(&r)).unwrap();
    let next = s.request("ping", None, &context()).unwrap();
    assert_ne!(id(&next), id(&r));
    s.cancel(&id(&next)).unwrap();
    assert_eq!(
        s.request("ping", None, &context()),
        Err(Error::RequestLimit)
    );
}
#[test]
fn oversize_request_does_not_consume_pending_slots_or_ids() {
    let mut s = ready(json!({}));
    let huge = obj(json!({"content":"x".repeat(1024*1024+1)}));
    assert!(s.request("ping", Some(huge), &context()).is_err());
    assert_eq!(s.pending_count(), 0);
    let r = s.request("ping", None, &context()).unwrap();
    assert_eq!(id(&r), Id::String("mcp-2".into()));
}
#[test]
fn server_requests_get_ping_or_method_not_found_without_host_execution() {
    let mut s = session();
    s.initialize(&context()).unwrap();
    for method in [
        "ping",
        "sampling/createMessage",
        "roots/list",
        "elicitation/create",
        "tools/call",
    ] {
        let raw = json!({"jsonrpc":"2.0","id":77,"method":method,"params":{"untrusted":"request"}});
        let Incoming::Respond(response) = s.receive(&serde_json::to_vec(&raw).unwrap()).unwrap()
        else {
            panic!()
        };
        if method == "ping" {
            assert!(matches!(
                response,
                Message::Success {
                    id: Id::Number(77),
                    ..
                }
            ));
        } else {
            assert!(matches!(
                response,
                Message::Failure {
                    id: Some(Id::Number(77)),
                    error: RpcError {
                        code: wire::METHOD_NOT_FOUND,
                        ..
                    }
                }
            ));
        }
        s.encode(&response).unwrap();
        assert_eq!(s.pending_count(), 1);
    }
}
#[test]
fn disconnect_drains_uncertain_requests_and_permanently_fences_the_session() {
    let mut s = ready(json!({"tools":{}}));
    let a = s.request("tools/call", None, &context()).unwrap();
    let b = s.request("ping", None, &context()).unwrap();
    let abandoned = s.close();
    assert_eq!(abandoned.len(), 2);
    assert!(
        abandoned
            .iter()
            .any(|r| r.id == id(&a) && r.method == "tools/call")
    );
    assert!(abandoned.iter().any(|r| r.id == id(&b)));
    assert_eq!(s.pending_count(), 0);
    assert!(s.close().is_empty());
    assert_eq!(s.request("ping", None, &context()), Err(Error::Phase));
    assert_eq!(s.receive(&success(&id(&a), json!({}))), Err(Error::Phase));
}
#[test]
fn typed_tool_and_content_payloads_keep_extensions_and_opaque_media() {
    let tool = json!({"name":"fixture","inputSchema":{"type":"object","properties":{}},"outputSchema":{"type":"object"},"annotations":{"readOnlyHint":false},"_meta":{"vendor":"data"}});
    let t: Tool = serde_json::from_value(tool.clone()).unwrap();
    assert_eq!(serde_json::to_value(t).unwrap(), tool);
    let content = json!({"content":[{"type":"text","text":"text","_meta":{"x":1}},{"type":"image","mimeType":"image/png","data":"opaque"},{"type":"audio","mimeType":"audio/wav","data":"opaque"},{"type":"resource_link","name":"reference","uri":"file:///untrusted"},{"type":"resource","resource":{"uri":"file:///untrusted","blob":"opaque"}}],"structuredContent":{"answer":42},"isError":false,"_meta":{"private":"data"}});
    let t: CallToolResult = serde_json::from_value(content.clone()).unwrap();
    assert_eq!(serde_json::to_value(t).unwrap(), content);
    let t: CallToolResult = serde_json::from_value(json!({"content":[]})).unwrap();
    assert!(!t.is_error);
    assert!(
        serde_json::from_value::<CallToolResult>(
            json!({"content":[{"type":"future-content","value":"unsupported"}]})
        )
        .is_err()
    );
}

#[test]
fn client_metadata_is_bounded_and_cannot_shadow_required_implementation_identity() {
    let mut info = Implementation::new("namzu", "1");
    info.extra.insert("name".into(), json!("shadow"));
    assert!(ClientSession::new(info, Limits::default()).is_err());
    let mut info = Implementation::new("namzu", "1");
    info.extra
        .insert("title".into(), json!("x".repeat(1024 * 1024 + 1)));
    assert!(ClientSession::new(info, Limits::default()).is_err());
    let mut info = Implementation::new("namzu", "1");
    info.extra
        .insert("title".into(), json!("bounded client metadata"));
    let mut s = ClientSession::new(info, Limits::default()).unwrap();
    let init = s.initialize(&context()).unwrap();
    let value: Value = serde_json::from_slice(&s.encode(&init).unwrap()).unwrap();
    assert_eq!(
        value["params"]["clientInfo"]["title"],
        "bounded client metadata"
    );
}
#[test]
fn null_or_wrong_type_known_capabilities_do_not_silently_negotiate() {
    for caps in [
        json!({"tools":null}),
        json!({"tools":{"listChanged":null}}),
        json!({"resources":{"subscribe":"yes"}}),
        json!({"prompts":[]}),
        json!({"logging":true}),
    ] {
        let mut s = session();
        let init = s.initialize(&context()).unwrap();
        assert_eq!(
            s.receive(&success(&id(&init), init_result(PREFERRED_VERSION, caps))),
            Err(Error::Initialize)
        );
        assert_eq!(s.phase(), Phase::Closed);
    }
}
#[test]
fn oversized_response_keeps_the_original_request_binding_and_cannot_become_a_reply() {
    let mut s = ready(json!({}));
    let request = s.request("ping", None, &context()).unwrap();
    assert_eq!(
        s.receive(&vec![b' '; 1024 * 1024 + 1]),
        Err(Error::MessageLimit)
    );
    assert_eq!(s.pending_count(), 1);
    assert!(matches!(
        s.receive(&success(&id(&request), json!({}))).unwrap(),
        Incoming::Reply { .. }
    ));
    assert_eq!(s.cancel(&id(&request)).unwrap(), None);
}
