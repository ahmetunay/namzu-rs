use super::*;
use namzu_core::{Cancellation, Limits, ToolCall, ToolResult, Tools, run_turn};
use std::{
    io::{Read, Write},
    net::TcpListener,
    time::Duration,
};

struct Request {
    path: String,
    body: Vec<u8>,
    headers: String,
}
type Handler = Box<dyn Fn(Request) -> (u16, &'static str, String) + Send>;
fn serve(handlers: Vec<Handler>) -> (Endpoints, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for handler in handlers {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut headers = vec![];
            while !headers.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                headers.push(byte[0]);
                assert!(headers.len() < 16384);
            }
            let headers = String::from_utf8(headers).unwrap();
            let path = headers
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .to_owned();
            let length = headers
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                        .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            assert!(length <= 1024 * 1024);
            let mut body = vec![0; length];
            stream.read_exact(&mut body).unwrap();
            let (status, kind, body) = handler(Request {
                path,
                body,
                headers,
            });
            write!(stream,"HTTP/1.1 {status} Fixture\r\nContent-Type: {kind}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",body.len(),body).unwrap();
        }
    });
    (
        Endpoints {
            auth: Url::parse(&format!("http://{address}/")).unwrap(),
            api: Url::parse(&format!("http://{address}/api/")).unwrap(),
        },
        server,
    )
}
fn http() -> Client {
    Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .build()
        .unwrap()
}
fn context() -> TurnContext {
    TurnContext::new(Duration::from_secs(30), Cancellation::default()).unwrap()
}
fn event(value: Value) -> String {
    format!("data: {value}\n\n")
}
fn completed(items: Value) -> String {
    event(json!({"type":"response.completed","response":{"status":"completed","output":items}}))
}
fn text_item(text: &str) -> Value {
    json!({"type":"message","id":"m-1","role":"assistant","status":"completed","content":[{"type":"output_text","text":text}]})
}

#[test]
fn device_login_exchanges_verifier_and_stores_private_tokens() {
    let root = tempfile::tempdir().unwrap();
    let credential = auth::fixture_credential("account", 4102444800);
    let token = credential.access_token.clone();
    let (endpoints, server) = serve(vec![
        Box::new(|request| {
            assert_eq!(request.path, "/api/accounts/deviceauth/usercode");
            assert_eq!(
                serde_json::from_slice::<Value>(&request.body).unwrap()["client_id"],
                auth::CLIENT_ID
            );
            (
                200,
                "application/json",
                json!({"device_auth_id":"device-fixture","user_code":"ABCD-1234","interval":0})
                    .to_string(),
            )
        }),
        Box::new(|_| (403, "application/json", "{}".into())),
        Box::new(|request| {
            assert_eq!(request.path, "/api/accounts/deviceauth/token");
            (
                200,
                "application/json",
                json!({"authorization_code":"code","code_verifier":"x".repeat(43)}).to_string(),
            )
        }),
        Box::new(move |request| {
            assert_eq!(request.path, "/oauth/token");
            let fields = url::form_urlencoded::parse(&request.body)
                .into_owned()
                .collect::<std::collections::BTreeMap<_, _>>();
            assert_eq!(fields["grant_type"], "authorization_code");
            assert_eq!(fields["code_verifier"], "x".repeat(43));
            (
                200,
                "application/json",
                json!({"access_token":token,"refresh_token":"new-refresh"}).to_string(),
            )
        }),
    ]);
    let mut observed = false;
    auth::login_with_client(root.path(), &endpoints, &http(), &context(), |url, code| {
        assert!(url.ends_with("/codex/device"));
        assert_eq!(code, "ABCD-1234");
        observed = true;
    })
    .unwrap();
    assert!(observed);
    server.join().unwrap();
    let status = auth::status(root.path()).unwrap();
    assert_eq!(status["authenticated"], true);
    assert_eq!(status["refresh_available"], true);
    assert!(!status.to_string().contains("new-refresh"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(root.path().join("codex-credentials.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn refresh_rotates_grant_once_and_new_calls_reuse_saved_credential() {
    let root = tempfile::tempdir().unwrap();
    auth::save_fixture(root.path(), &auth::fixture_credential("account", 1));
    let token = auth::fixture_credential("account", 4102444800).access_token;
    let (endpoints, server) = serve(vec![Box::new(move |request| {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["grant_type"], "refresh_token");
        assert_eq!(body["refresh_token"], "fixture-refresh");
        (
            200,
            "application/json",
            json!({"access_token":token,"refresh_token":"rotated"}).to_string(),
        )
    })]);
    auth::ensure_fresh(root.path(), &endpoints, &http(), &context()).unwrap();
    server.join().unwrap();
    let value = auth::ensure_fresh(root.path(), &endpoints, &http(), &context()).unwrap();
    assert_eq!(value.account_id, "account");
    let stored: Value = namzu_session::PrivateStore::open(root.path())
        .unwrap()
        .read("codex-credentials.json")
        .unwrap()
        .unwrap();
    assert_eq!(stored["refresh_token"], "rotated");
}

#[test]
fn refresh_refuses_account_switch_without_overwriting_saved_grant() {
    let root = tempfile::tempdir().unwrap();
    auth::save_fixture(root.path(), &auth::fixture_credential("original", 1));
    let token = auth::fixture_credential("other", 4102444800).access_token;
    let (endpoints, server) = serve(vec![Box::new(move |_| {
        (
            200,
            "application/json",
            json!({"access_token":token}).to_string(),
        )
    })]);
    assert!(auth::ensure_fresh(root.path(), &endpoints, &http(), &context()).is_err());
    server.join().unwrap();
    let stored: Value = namzu_session::PrivateStore::open(root.path())
        .unwrap()
        .read("codex-credentials.json")
        .unwrap()
        .unwrap();
    assert_eq!(stored["account_id"], "original");
}

#[test]
fn cancelled_device_login_stores_nothing_and_logout_is_locked() {
    let root = tempfile::tempdir().unwrap();
    let cancellation = Cancellation::default();
    let signal = cancellation.clone();
    let (endpoints, server) = serve(vec![Box::new(|_| {
        (
            200,
            "application/json",
            json!({"device_auth_id":"device","user_code":"1234","interval":1}).to_string(),
        )
    })]);
    let result = auth::login_with_client(
        root.path(),
        &endpoints,
        &http(),
        &TurnContext::new(Duration::from_secs(30), cancellation).unwrap(),
        |_, _| signal.cancel(),
    );
    assert_eq!(result.unwrap_err(), AgentError::Cancelled);
    server.join().unwrap();
    assert_eq!(auth::status(root.path()).unwrap()["authenticated"], false);
    auth::save_fixture(
        root.path(),
        &auth::fixture_credential("account", 4102444800),
    );
    let store = namzu_session::PrivateStore::open(root.path()).unwrap();
    let lock = store.lock("codex-auth").unwrap();
    assert!(auth::logout(root.path()).is_err());
    drop(lock);
    auth::logout(root.path()).unwrap();
    assert_eq!(auth::status(root.path()).unwrap()["authenticated"], false);
}

#[test]
fn incomplete_or_truncated_sse_never_yields_a_tool_effect() {
    for stream in [
        event(json!({"type":"response.incomplete","response":{"status":"incomplete"}})),
        event(
            json!({"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","call_id":"x","name":"effect","arguments":"{}"}}),
        ),
        "data: [DONE]\n\n".into(),
    ] {
        assert!(sse::parse(std::io::Cursor::new(stream), &context()).is_err());
    }
}

#[test]
fn finalized_items_fallback_preserves_reasoning_and_function_call_id() {
    let reasoning = json!({"type":"reasoning","id":"r1","encrypted_content":"opaque-fixture"});
    let call = json!({"type":"function_call","id":"item-id","call_id":"call-id","name":"effect","arguments":"{}"});
    let stream =
        event(json!({"type":"response.output_item.done","output_index":0,"item":reasoning}))
            + &event(json!({"type":"response.output_item.done","output_index":1,"item":call}))
            + &completed(json!([]));
    let (completion, items) = sse::parse(std::io::Cursor::new(stream), &context()).unwrap();
    assert_eq!(completion.tool_calls[0].id, "call-id");
    assert_eq!(items[0]["encrypted_content"], "opaque-fixture");
}

#[test]
fn malformed_arguments_and_duplicate_finalized_indices_are_refused() {
    let call = json!({"type":"function_call","call_id":"c","name":"effect","arguments":"[]"});
    assert!(sse::parse(std::io::Cursor::new(completed(json!([call]))), &context()).is_err());
    let stream =
        event(json!({"type":"response.output_item.done","output_index":0,"item":text_item("a")}));
    assert!(
        sse::parse(
            std::io::Cursor::new(stream.clone() + &stream + &completed(json!([]))),
            &context()
        )
        .is_err()
    );
}

#[test]
fn native_http_tool_roundtrip_replays_opaque_reasoning_and_correct_ids() {
    struct Effect(usize);
    impl Tools for Effect {
        fn definitions(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                name: "effect".into(),
                description: "fixture".into(),
                parameters: json!({"type":"object","properties":{},"additionalProperties":false}),
            }]
        }
        fn call(&mut self, call: &ToolCall, _: &TurnContext) -> Result<ToolResult, AgentError> {
            assert_eq!(call.id, "call-1");
            self.0 += 1;
            Ok(ToolResult::ok("12"))
        }
    }
    let root = tempfile::tempdir().unwrap();
    auth::save_fixture(
        root.path(),
        &auth::fixture_credential("account", 4102444800),
    );
    let (endpoints, server) = serve(vec![
        Box::new(|request| {
            assert_eq!(request.path, "/api/responses");
            assert!(
                request
                    .headers
                    .to_ascii_lowercase()
                    .contains("chatgpt-account-id: account")
            );
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["store"], false);
            assert_eq!(body["tools"][0]["type"], "function");
            (
                200,
                "text/event-stream",
                completed(
                    json!([{"type":"reasoning","id":"r1","encrypted_content":"opaque-fixture"},{"type":"function_call","id":"fc1","call_id":"call-1","name":"effect","arguments":"{}"}]),
                ),
            )
        }),
        Box::new(|request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["input"][1]["type"], "reasoning");
            assert_eq!(body["input"][1]["encrypted_content"], "opaque-fixture");
            assert_eq!(body["input"][2]["call_id"], "call-1");
            assert_eq!(body["input"][3]["type"], "function_call_output");
            assert_eq!(body["input"][3]["call_id"], "call-1");
            (
                200,
                "text/event-stream",
                completed(json!([text_item("12")])),
            )
        }),
    ]);
    let mut driver = CodexProvider::new(
        root.path().into(),
        "fixture-model".into(),
        Some("low".into()),
    )
    .unwrap();
    driver.endpoints = endpoints;
    driver.http = http();
    let mut tools = Effect(0);
    let result = run_turn(
        &mut driver,
        &mut tools,
        "7+5",
        Limits::default(),
        Cancellation::default(),
    )
    .unwrap();
    server.join().unwrap();
    assert_eq!(result.output, "12");
    assert_eq!(tools.0, 1);
    assert!(
        result
            .messages
            .iter()
            .any(|message| matches!(message, Message::ProviderState { .. }))
    );
}

#[test]
fn replay_state_is_not_reused_on_another_route_or_after_message_edit() {
    let message = Message::Assistant {
        content: "answer".into(),
        tool_calls: vec![],
    };
    let state = json!({"kind":"namzu.codex.responses","route":"route-1","signature":{"text":"answer","tool_calls":[]},"items":[{"type":"reasoning","encrypted_content":"opaque"},text_item("answer")]});
    let messages = vec![
        Message::ProviderState {
            data: state.clone(),
        },
        message.clone(),
    ];
    assert_eq!(encode_input(&messages, "route-1").unwrap().len(), 2);
    assert_eq!(encode_input(&messages, "route-2").unwrap().len(), 1);
    let changed = vec![
        Message::ProviderState { data: state },
        Message::Assistant {
            content: "edited".into(),
            tool_calls: vec![],
        },
    ];
    assert_eq!(encode_input(&changed, "route-1").unwrap().len(), 1);
    let public = crate::encode_messages(&messages);
    assert_eq!(public.len(), 1);
    assert_eq!(public[0]["content"], "answer");
}

#[test]
fn model_catalogue_uses_authenticated_route_and_filters_hidden_models() {
    let root = tempfile::tempdir().unwrap();
    auth::save_fixture(
        root.path(),
        &auth::fixture_credential("account", 4102444800),
    );
    let (endpoints, server) = serve(vec![Box::new(|request| {
        assert_eq!(request.path, "/api/models?client_version=1.0.0");
        (200,"application/json",json!({"models":[{"slug":"fixture-luna","display_name":"Fixture Luna","visibility":"list","supported_reasoning_levels":[{"effort":"low"}]},{"slug":"hidden","visibility":"hide"}]}).to_string())
    })]);
    let mut driver = CodexProvider::new(root.path().into(), "catalogue".into(), None).unwrap();
    driver.endpoints = endpoints;
    driver.http = http();
    let models = driver.models(&context()).unwrap();
    server.join().unwrap();
    assert_eq!(models["models"].as_array().unwrap().len(), 1);
    assert_eq!(models["models"][0]["id"], "fixture-luna");
}
