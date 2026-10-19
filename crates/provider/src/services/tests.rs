use super::*;
use crate::OpenAiCompatible;
use namzu_core::{
    Cancellation, Event, EventSink, Message, Provider, ToolCall, ToolDefinition, TurnContext,
};
use serde_json::json;
use std::{
    io::{Read, Write},
    net::TcpListener,
    time::{Duration, Instant},
};
fn ctx() -> TurnContext {
    TurnContext::new(Duration::from_secs(15), Cancellation::default()).unwrap()
}
fn chat(message: Value) -> Value {
    json!({"choices":[{"finish_reason":if message.get("tool_calls").is_some(){"tool_calls"}else{"stop"},"message":message}]})
}
fn event(v: Value) -> String {
    format!("data: {v}\n\n")
}
fn tools() -> Vec<ToolDefinition> {
    vec![ToolDefinition {
        name: "read".into(),
        description: "fixture".into(),
        parameters: json!({"type":"object"}),
    }]
}
fn user() -> Vec<Message> {
    vec![Message::User {
        content: "fixture".into(),
    }]
}
struct Request {
    path: String,
    headers: String,
    body: Value,
}
type Handler = Box<dyn FnOnce(Request) -> (u16, &'static str, String) + Send>;
fn serve(handlers: Vec<Handler>) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let worker = std::thread::spawn(move || {
        for handler in handlers {
            let until = Instant::now() + Duration::from_secs(10);
            let mut s = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < until, "missing service fixture request");
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut bytes = vec![];
            let mut b = [0];
            while !bytes.ends_with(b"\r\n\r\n") {
                s.read_exact(&mut b).unwrap();
                bytes.push(b[0]);
                assert!(bytes.len() < 16384);
            }
            let headers = String::from_utf8(bytes).unwrap();
            let size = headers
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|n| n.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            let mut bytes = vec![0; size];
            s.read_exact(&mut bytes).unwrap();
            let path = headers
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .to_string();
            let (status, kind, body) = handler(Request {
                path,
                headers,
                body: serde_json::from_slice(&bytes).unwrap(),
            });
            write!(s,"HTTP/1.1 {status} Fixture\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        }
    });
    (format!("http://{addr}/v1"), worker)
}
fn provider(service: Service, url: &str, model: &str, options: Options) -> OpenAiCompatible {
    let mut p =
        OpenAiCompatible::for_service(service, url, model.into(), "fixture-only-key", options)
            .unwrap();
    p.client = crate::http::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    p
}
#[derive(Default)]
struct Events(Vec<Event>);
impl EventSink for Events {
    fn emit(&mut self, e: Event) -> Result<(), AgentError> {
        self.0.push(e);
        Ok(())
    }
}
#[test]
fn profiles_and_wire_selection_are_explicit() {
    assert_eq!(
        Service::Openrouter.base_url(),
        "https://openrouter.ai/api/v1"
    );
    assert_eq!(Service::ZenGo.base_url(), "https://opencode.ai/zen/go/v1");
    assert_eq!(Service::Zen.key_env(), "OPENCODE_API_KEY");
    assert_eq!(
        Options::default().resolved(Service::Zen, "gpt-6").unwrap(),
        Format::Responses
    );
    assert_eq!(
        Options::default()
            .resolved(Service::ZenGo, "deepseek-v4-pro")
            .unwrap(),
        Format::Chat
    );
    for model in ["claude-x", "gemini-x"] {
        assert!(Options::default().resolved(Service::Zen, model).is_err());
    }
    assert!(
        Options {
            format: Format::Responses,
            ..Default::default()
        }
        .resolved(Service::Openrouter, "fixture")
        .is_err()
    );
    assert!(Service::parse("unknown").is_err());
}
#[test]
fn options_reject_unknown_unhonored_and_unsafe_values() {
    assert!(serde_json::from_value::<Options>(json!({"models":["paid-fallback"]})).is_err());
    for max in [0, 1_000_001] {
        assert!(
            Options {
                max_output_tokens: Some(max),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }
    for value in [
        "https://user:secret@example.com",
        "https://example.com/?key=secret",
        "file:///tmp/x",
    ] {
        assert!(
            Options {
                site_url: Some(value.into()),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }
    assert!(
        Options {
            site_name: Some("bad\nheader".into()),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        Options {
            site_url: Some("https://example.com".into()),
            ..Default::default()
        }
        .resolved(Service::Zen, "model")
        .is_err()
    );
}
#[test]
fn decimal_cost_is_exact_nonnegative_and_never_rounds_positive_to_free() {
    for (raw, want) in [
        ("0", 0),
        ("0.000000001", 1),
        ("0.0000000001", 1),
        ("1.234567891", 1_234_567_891),
        ("1.2345678911", 1_234_567_892),
        ("5e-10", 1),
        ("2e1", 20_000_000_000),
    ] {
        let v: Value = serde_json::from_str(raw).unwrap();
        assert_eq!(reported_cost(&v).unwrap().unwrap().nano_usd, want);
        assert_eq!(
            reported_cost(&Value::String(raw.into()))
                .unwrap()
                .unwrap()
                .nano_usd,
            want
        );
    }
    assert!(reported_cost(&Value::Null).unwrap().is_none());
    for bad in [
        "-1",
        "NaN",
        "inf",
        "1e129",
        "18446744073709551616",
        "0.1.2",
        "",
    ] {
        assert!(reported_cost(&Value::String(bad.into())).is_err());
    }
}
#[test]
fn openrouter_headers_no_fallback_cost_and_reasoning_replay() {
    let native =
        json!({"type":"reasoning.encrypted","data":"opaque-fixture","id":"r","format":"fixture"});
    let replay = native.clone();
    let (url, worker) = serve(vec![
        Box::new(move |r| {
            assert_eq!(r.path, "/v1/chat/completions");
            assert!(
                r.headers
                    .to_ascii_lowercase()
                    .contains("http-referer: https://example.com/")
            );
            assert!(r.headers.to_ascii_lowercase().contains("x-title: fixture"));
            assert_eq!(
                r.body["provider"],
                json!({"allow_fallbacks":false,"require_parameters":true})
            );
            assert_eq!(r.body["reasoning"]["effort"], "low");
            assert_eq!(r.body["max_tokens"], 128);
            let mut v = chat(
                json!({"role":"assistant","content":null,"reasoning_details":[native],"tool_calls":[{"id":"c","type":"function","function":{"name":"read","arguments":"{}"}}]}),
            );
            v["usage"] = json!({"prompt_tokens":3,"completion_tokens":2,"total_tokens":5,"cost":0.0000000011});
            (200, "application/json", v.to_string())
        }),
        Box::new(move |r| {
            assert_eq!(r.body["messages"][1]["reasoning_details"][0], replay);
            assert!(!r.body.to_string().contains("credential_tag"));
            (
                200,
                "application/json",
                chat(json!({"role":"assistant","content":"done"})).to_string(),
            )
        }),
    ]);
    let options = Options {
        site_url: Some("https://example.com/".into()),
        site_name: Some("fixture".into()),
        reasoning_effort: Some("low".into()),
        max_output_tokens: Some(128),
        ..Default::default()
    };
    let mut p = provider(Service::Openrouter, &url, "vendor/model", options.clone());
    let answer = p.complete(&user(), &tools(), &ctx()).unwrap();
    assert_eq!(p.take_metadata().unwrap().cost.unwrap().nano_usd, 2);
    let state = p.take_state().unwrap();
    assert!(!state.to_string().contains("fixture-only-key"));
    let mut history = user();
    history.push(Message::ProviderState { data: state });
    history.push(Message::Assistant {
        content: answer.text,
        tool_calls: answer.tool_calls,
    });
    history.push(Message::Tool {
        tool_call_id: "c".into(),
        content: "read".into(),
        is_error: false,
    });
    p.complete(&history, &tools(), &ctx()).unwrap();
    worker.join().unwrap();
    let mut other = OpenAiCompatible::for_service(
        Service::Openrouter,
        &url,
        "vendor/model".into(),
        "different-fixture-key",
        options,
    )
    .unwrap();
    assert!(matches!(
        other.complete(&history, &tools(), &ctx()),
        Err(AgentError::Configuration(_))
    ));
}
#[test]
fn fragmented_chat_stream_keeps_tools_inert_and_reasoning_private() {
    let stream = event(
        json!({"choices":[{"index":0,"delta":{"role":"assistant","content":"an","reasoning_details":[{"index":0,"type":"reasoning.encrypted","data":"opa"}],"tool_calls":[{"index":0,"id":"c","type":"function","function":{"name":"re","arguments":"{"}}]},"finish_reason":null}]}),
    ) + &event(
        json!({"choices":[{"index":0,"delta":{"content":"swer","reasoning_details":[{"index":0,"data":"que"}],"tool_calls":[{"index":0,"function":{"name":"ad","arguments":"}"}}]},"finish_reason":"tool_calls"}]}),
    ) + &event(
        json!({"choices":[],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5,"cost":"0.01"}}),
    ) + "data: [DONE]\n\n";
    let (url, worker) = serve(vec![Box::new(move |r| {
        assert_eq!(r.body["stream"], true);
        assert_eq!(r.body["stream_options"]["include_usage"], true);
        (200, "text/event-stream", stream)
    })]);
    let mut p = provider(
        Service::Openrouter,
        &url,
        "fixture",
        Options {
            stream: true,
            ..Default::default()
        },
    );
    let mut events = Events::default();
    let out = p
        .complete_stream(&user(), &tools(), &ctx(), &mut events)
        .unwrap();
    assert_eq!(out.text, "answer");
    assert_eq!(
        out.tool_calls[0],
        ToolCall {
            id: "c".into(),
            name: "read".into(),
            arguments: json!({})
        }
    );
    assert_eq!(
        p.take_metadata().unwrap().cost.unwrap().nano_usd,
        10_000_000
    );
    assert_eq!(
        p.take_state().unwrap()["reasoning_details"][0]["data"],
        "opaque"
    );
    assert!(
        events
            .0
            .iter()
            .all(|e| !serde_json::to_string(e).unwrap().contains("opaque"))
    );
    worker.join().unwrap();
}
#[test]
fn truncated_or_error_streams_never_commit_metadata_or_tool_calls() {
    for stream in [
        event(json!({"choices":[{"delta":{"content":"partial"},"finish_reason":"stop"}]})),
        event(json!({"error":{"code":429,"message":"PRIVATE_ERROR"}})),
        event(
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"read","arguments":"{"}}]},"finish_reason":"tool_calls"}]}),
        ) + "data: [DONE]\n\n",
    ] {
        let (url, worker) = serve(vec![Box::new(move |_| (200, "text/event-stream", stream))]);
        let mut p = provider(
            Service::Openrouter,
            &url,
            "fixture",
            Options {
                stream: true,
                ..Default::default()
            },
        );
        let error = p.complete(&user(), &tools(), &ctx()).unwrap_err();
        assert!(!error.to_string().contains("PRIVATE_ERROR"));
        assert!(p.take_metadata().is_none());
        assert!(p.take_state().is_none());
        worker.join().unwrap();
    }
}
#[test]
fn zen_responses_route_native_items_and_usage_roundtrip() {
    let (url, worker) = serve(vec![
        Box::new(|r| {
            assert_eq!(r.path, "/v1/responses");
            assert_eq!(r.body["model"], "gpt-6");
            assert_eq!(r.body["store"], false);
            assert_eq!(r.body["max_output_tokens"], 64);
            assert!(r.body.get("messages").is_none());
            assert_eq!(r.body["instructions"], "fixture-system");
            (200,"application/json",json!({"status":"completed","output":[{"type":"reasoning","encrypted_content":"private-reasoning"},{"type":"message","content":[{"type":"output_text","text":"ok"}]}],"usage":{"input_tokens":1,"output_tokens":2,"total_tokens":3}}).to_string())
        }),
        Box::new(|r| {
            assert!(
                r.body["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|i| i["encrypted_content"] == "private-reasoning")
            );
            (200,"application/json",json!({"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"final"}]}]}).to_string())
        }),
    ]);
    let mut p = provider(
        Service::Zen,
        &url,
        "zen/gpt-6",
        Options {
            max_output_tokens: Some(64),
            ..Default::default()
        },
    );
    let mut history = vec![Message::System {
        content: "fixture-system".into(),
    }];
    history.extend(user());
    let out = p.complete(&history, &[], &ctx()).unwrap();
    assert_eq!(out.text, "ok");
    assert_eq!(
        p.take_metadata().unwrap().usage.unwrap().total_tokens,
        Some(3)
    );
    history.push(Message::ProviderState {
        data: p.take_state().unwrap(),
    });
    history.push(Message::Assistant {
        content: out.text,
        tool_calls: vec![],
    });
    history.push(Message::User {
        content: "continue".into(),
    });
    assert_eq!(p.complete(&history, &[], &ctx()).unwrap().text, "final");
    worker.join().unwrap();
}
#[test]
fn zen_go_chat_fills_missing_reasoning_without_switching_endpoint() {
    let (url, worker) = serve(vec![Box::new(|r| {
        assert_eq!(r.path, "/v1/chat/completions");
        assert_eq!(r.body["model"], "deepseek-v4-pro");
        assert_eq!(r.body["messages"][0]["reasoning_content"], "");
        (
            200,
            "application/json",
            chat(json!({"role":"assistant","content":"ok","reasoning_content":"captured"}))
                .to_string(),
        )
    })]);
    let mut p = provider(
        Service::ZenGo,
        &url,
        "go/deepseek-v4-pro",
        Options::default(),
    );
    p.complete(
        &[Message::Assistant {
            content: "history".into(),
            tool_calls: vec![],
        }],
        &[],
        &ctx(),
    )
    .unwrap();
    assert_eq!(p.take_state().unwrap()["reasoning_content"], "captured");
    worker.join().unwrap();
}
#[test]
fn stream_boundaries_duplicate_finish_and_disconnection_are_fail_closed() {
    for input in [
        "data: [DONE]\n\n".into(),
        event(json!({"choices":[{"delta":{},"finish_reason":"stop"}]}))
            + &event(json!({"choices":[{"delta":{"content":"late"},"finish_reason":null}]}))
            + "data: [DONE]\n\n",
        format!("data: {}\n\n", "x".repeat(1024 * 1024 + 1)),
    ] {
        assert!(
            crate::chat_stream::parse(
                std::io::Cursor::new(input),
                &ctx(),
                &mut Events::default(),
                false
            )
            .is_err()
        );
    }
    struct Gone;
    impl EventSink for Gone {
        fn emit(&mut self, _: Event) -> Result<(), AgentError> {
            Err(AgentError::Cancelled)
        }
    }
    let stream = event(json!({"choices":[{"delta":{"content":"x"},"finish_reason":null}]}));
    assert_eq!(
        crate::chat_stream::parse(std::io::Cursor::new(stream), &ctx(), &mut Gone, false),
        Err(AgentError::Cancelled)
    );
}
#[test]
fn foreign_reasoning_is_not_forwarded_and_http200_error_is_typed() {
    let (url, worker) = serve(vec![Box::new(|r| {
        assert!(!r.body.to_string().contains("PRIVATE_REASONING"));
        (
            200,
            "application/json",
            json!({"error":{"code":402,"message":"PRIVATE_ERROR"}}).to_string(),
        )
    })]);
    let mut p = provider(Service::OpenaiCompatible, &url, "model", Options::default());
    let error=p.complete(&[Message::ProviderState{data:json!({"kind":"namzu.api.chat","route":"foreign","reasoning_content":"PRIVATE_REASONING"})},Message::Assistant{content:"hello".into(),tool_calls:vec![]}],&[],&ctx()).unwrap_err();
    assert_eq!(error.provider_failure().unwrap().http_status, Some(402));
    assert!(!error.to_string().contains("PRIVATE_ERROR"));
    worker.join().unwrap();
}

#[test]
fn api_key_responses_stream_requires_completed_output_and_clears_on_cancel() {
    let stream = event(json!({"type":"response.output_text.delta","delta":"ready"}))
        + &event(
            json!({"type":"response.completed","response":{"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"ready"}]}],"usage":{"input_tokens":4,"output_tokens":1,"total_tokens":5}}}),
        );
    let (url, worker) = serve(vec![Box::new(move |r| {
        assert_eq!(r.path, "/v1/responses");
        assert_eq!(r.body["stream"], true);
        (200, "text/event-stream", stream)
    })]);
    let mut p = provider(
        Service::Zen,
        &url,
        "gpt-6",
        Options {
            stream: true,
            ..Default::default()
        },
    );
    assert_eq!(p.complete(&user(), &[], &ctx()).unwrap().text, "ready");
    assert!(p.take_metadata().is_some());
    worker.join().unwrap();
    let cancelled = ctx();
    cancelled.cancellation().cancel();
    assert!(matches!(
        p.complete(&user(), &[], &cancelled),
        Err(AgentError::Cancelled)
    ));
    assert!(p.take_metadata().is_none());
    assert!(p.take_state().is_none());
}

#[test]
fn wire_cost_preserves_decimal_lexeme_without_changing_global_json_numbers() {
    // A float conversion would erase the final digit and undercount this charge.
    for raw in ["1.00000000000000000001", "0.00000000100000000000000001"] {
        let bytes = format!(r#"{{"usage":{{"cost":{raw}}}}}"#);
        let expected = if raw.starts_with('1') {
            1_000_000_001
        } else {
            2
        };
        assert_eq!(
            raw_cost(bytes.as_bytes()).unwrap().unwrap().nano_usd,
            expected
        );
    }
    for raw in ["true", "{}", "[]", "-0.1", "1e999"] {
        assert!(raw_cost(format!(r#"{{"usage":{{"cost":{raw}}}}}"#).as_bytes()).is_err());
    }
    assert!(raw_cost(br#"{"usage":{"cost":null}}"#).unwrap().is_none());
    assert!(serde_json::from_str::<Value>("1e999").is_err());
    let stream = event(json!({"choices":[{"delta":{},"finish_reason":"stop"}]}))
        + "data: {\"choices\":[],\"usage\":{\"cost\":1.00000000000000000001}}\n\n"
        + "data: {\"choices\":[],\"usage\":{\"cost\":1.00000000000000000000}}\n\n"
        + "data: [DONE]\n\n";
    assert_eq!(
        crate::chat_stream::parse(
            std::io::Cursor::new(stream),
            &ctx(),
            &mut Events::default(),
            true
        ),
        Err(AgentError::Protocol("conflicting reported cost chunks"))
    );
}
