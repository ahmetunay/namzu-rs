use super::*;
use crate::services::Options;
use namzu_core::{Cancellation, NoopEvents, Provider};
use std::{
    io::{Read, Write},
    net::TcpListener,
    time::{Duration, Instant},
};
fn ctx() -> TurnContext {
    TurnContext::new(Duration::from_secs(15), Cancellation::default()).unwrap()
}
fn messages() -> Vec<Message> {
    vec![
        Message::System {
            content: "fixture instructions".into(),
        },
        Message::User {
            content: "fixture".into(),
        },
    ]
}
fn tools() -> Vec<ToolDefinition> {
    vec![ToolDefinition {
        name: "read".into(),
        description: "fixture tool".into(),
        parameters: json!({"type":"object"}),
    }]
}
type Handler = Box<dyn FnOnce(String, String, Value) -> (u16, String) + Send>;
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
                        assert!(Instant::now() < until, "missing fixture request");
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut raw = vec![];
            while !raw.ends_with(b"\r\n\r\n") {
                let mut b = [0];
                s.read_exact(&mut b).unwrap();
                raw.push(b[0]);
                assert!(raw.len() < 32768);
            }
            let headers = String::from_utf8(raw).unwrap();
            let size = headers
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|n| n.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            let mut raw = vec![0; size];
            s.read_exact(&mut raw).unwrap();
            let path = headers
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .to_owned();
            let (status, body) = handler(path, headers, serde_json::from_slice(&raw).unwrap());
            write!(s,"HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        }
    });
    (format!("http://{addr}"), worker)
}
fn provider(service: Service, url: &str, model: &str) -> OpenAiCompatible {
    let mut p = OpenAiCompatible::for_service(
        service,
        url,
        model.into(),
        if service.key_optional() {
            ""
        } else {
            "fixture-key"
        },
        Options::default(),
    )
    .unwrap();
    p.client = crate::http::Client::builder().no_proxy().build().unwrap();
    p
}
fn reply(f: Format, call: bool) -> Value {
    match f {
        Format::Messages => {
            json!({"type":"message","role":"assistant","stop_reason":if call{"tool_use"}else{"end_turn"},"content":if call{json!([{"type":"thinking","thinking":"PRIVATE_THOUGHT","signature":"signature"},{"type":"tool_use","id":"native-call","name":"read","input":{}}])}else{json!([{"type":"text","text":"answer"}])},"usage":{"input_tokens":3,"output_tokens":2}})
        }
        Format::Google => {
            json!({"candidates":[{"finishReason":"STOP","content":{"role":"model","parts":if call{json!([{"functionCall":{"name":"read","args":{}},"thoughtSignature":"PRIVATE_SIGNATURE"}])}else{json!([{"text":"answer"}])}}}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2,"totalTokenCount":5}})
        }
        Format::Converse => {
            json!({"stopReason":if call{"tool_use"}else{"end_turn"},"output":{"message":{"role":"assistant","content":if call{json!([{"reasoningContent":{"reasoningText":{"text":"PRIVATE_THOUGHT","signature":"signature"}}},{"toolUse":{"toolUseId":"native-call","name":"read","input":{}}}])}else{json!([{"text":"answer"}])}}},"usage":{"inputTokens":3,"outputTokens":2,"totalTokens":5}})
        }
        Format::Ollama => {
            json!({"done":true,"done_reason":"stop","message":{"role":"assistant","content":if call{""}else{"answer"},"tool_calls":if call{json!([{"function":{"name":"read","arguments":{}}}])}else{json!([])}},"prompt_eval_count":3,"eval_count":2})
        }
        _ => unreachable!(),
    }
}
fn roundtrip(service: Service, model: &str, f: Format, path: &'static str) {
    let (url, worker) = serve(vec![
        Box::new(move |actual, headers, body| {
            assert_eq!(actual, path);
            let headers = headers.to_ascii_lowercase();
            match service {
                Service::Anthropic | Service::Zen if f == Format::Messages => {
                    assert!(headers.contains("x-api-key: fixture-key"));
                    assert!(headers.contains("anthropic-version: 2023-06-01"));
                    assert!(!headers.contains("authorization:"));
                }
                Service::Google | Service::Zen if f == Format::Google => {
                    assert!(headers.contains("x-goog-api-key: fixture-key"));
                    assert!(!headers.contains("authorization:"));
                }
                Service::Ollama => {
                    assert!(!headers.contains("authorization:"));
                    assert_eq!(body["think"], false);
                }
                _ => assert!(headers.contains("authorization: bearer fixture-key")),
            }
            match f {
                Format::Messages => {
                    assert_eq!(body["system"][0]["text"], "fixture instructions");
                    assert_eq!(body["tools"][0]["input_schema"], json!({"type":"object"}));
                }
                Format::Google => {
                    assert_eq!(
                        body["systemInstruction"]["parts"][0]["text"],
                        "fixture instructions"
                    );
                    assert_eq!(body["tools"][0]["functionDeclarations"][0]["name"], "read");
                }
                Format::Converse => {
                    assert_eq!(body["system"][0]["text"], "fixture instructions");
                    assert_eq!(
                        body["toolConfig"]["tools"][0]["toolSpec"]["inputSchema"]["json"],
                        json!({"type":"object"})
                    );
                }
                _ => (),
            }
            (200, reply(f, true).to_string())
        }),
        Box::new(move |_, _, body| {
            assert!(!body.to_string().contains("credential_tag"));
            match f {
                Format::Messages => {
                    assert_eq!(
                        body["messages"][1]["content"][0]["thinking"],
                        "PRIVATE_THOUGHT"
                    );
                    assert_eq!(
                        body["messages"][2]["content"][0]["tool_use_id"],
                        "native-call"
                    );
                    assert_eq!(body["messages"][2]["content"][0]["is_error"], true);
                }
                Format::Google => {
                    assert_eq!(
                        body["contents"][1]["parts"][0]["thoughtSignature"],
                        "PRIVATE_SIGNATURE"
                    );
                    let r = &body["contents"][2]["parts"][0]["functionResponse"];
                    assert_eq!(r["name"], "read");
                    assert!(r.get("id").is_none());
                    assert_eq!(r["response"]["error"], "fixture result");
                }
                Format::Converse => {
                    assert_eq!(
                        body["messages"][1]["content"][0]["reasoningContent"]["reasoningText"]["text"],
                        "PRIVATE_THOUGHT"
                    );
                    assert_eq!(
                        body["messages"][2]["content"][0]["toolResult"]["status"],
                        "error"
                    );
                }
                Format::Ollama => {
                    assert_eq!(body["messages"][3]["tool_name"], "read");
                }
                _ => (),
            }
            (200, reply(f, false).to_string())
        }),
    ]);
    let mut p = provider(service, &url, model);
    let c = p.complete(&messages(), &tools(), &ctx()).unwrap();
    assert_eq!(c.tool_calls.len(), 1);
    assert!(c.text.is_empty());
    assert_eq!(
        p.take_metadata().unwrap().usage.unwrap().total_tokens,
        Some(5)
    );
    let state = p.take_state();
    let mut history = messages();
    if let Some(state) = &state {
        assert!(!state.to_string().contains("fixture-key"));
        history.push(Message::ProviderState {
            data: state.clone(),
        });
    }
    history.push(Message::Assistant {
        content: c.text,
        tool_calls: c.tool_calls.clone(),
    });
    history.push(Message::Tool {
        tool_call_id: c.tool_calls[0].id.clone(),
        content: "fixture result".into(),
        is_error: true,
    });
    assert_eq!(
        p.complete(&history, &tools(), &ctx()).unwrap().text,
        "answer"
    );
    worker.join().unwrap();
    if state.is_some() {
        let mut other = OpenAiCompatible::for_service(
            service,
            &url,
            model.into(),
            "different-key",
            Options::default(),
        )
        .unwrap();
        assert!(matches!(
            other.complete(&history, &tools(), &ctx()),
            Err(AgentError::Configuration(_))
        ));
    }
}
#[test]
fn anthropic_native_tool_thinking_usage_replay() {
    roundtrip(
        Service::Anthropic,
        "claude-fixture",
        Format::Messages,
        "/messages",
    );
}
#[test]
fn google_native_tool_ids_signatures_usage_replay() {
    roundtrip(
        Service::Google,
        "models/gemini-fixture",
        Format::Google,
        "/models/gemini-fixture:generateContent",
    );
}
#[test]
fn bedrock_converse_tool_thinking_usage_replay() {
    roundtrip(
        Service::Bedrock,
        "anthropic.fixture:0",
        Format::Converse,
        "/model/anthropic.fixture:0/converse",
    );
}
#[test]
fn ollama_keyless_native_tool_results() {
    roundtrip(Service::Ollama, "fixture", Format::Ollama, "/api/chat");
}
#[test]
fn zen_anthropic_native_keyed_route() {
    roundtrip(
        Service::Zen,
        "zen/claude-fixture",
        Format::Messages,
        "/messages",
    );
}
#[test]
fn zen_google_native_keyed_route() {
    roundtrip(
        Service::Zen,
        "zen/gemini-fixture",
        Format::Google,
        "/models/gemini-fixture:generateContent",
    );
}
#[test]
fn native_bad_finish_and_call_shapes_fail_before_metadata() {
    for f in [
        Format::Messages,
        Format::Google,
        Format::Converse,
        Format::Ollama,
    ] {
        let mut r = reply(f, true);
        match f {
            Format::Messages => r["stop_reason"] = "max_tokens".into(),
            Format::Google => r["candidates"][0]["finishReason"] = "MAX_TOKENS".into(),
            Format::Converse => r["stopReason"] = "max_tokens".into(),
            Format::Ollama => r["done"] = false.into(),
            _ => (),
        }
        assert!(decode(&r, f, "fixture").is_err());
        let mut r = reply(f, true);
        match f {
            Format::Messages => r["content"][1]["input"] = false.into(),
            Format::Google => {
                r["candidates"][0]["content"]["parts"][0]["functionCall"]["args"] = false.into()
            }
            Format::Converse => {
                r["output"]["message"]["content"][1]["toolUse"]["input"] = false.into()
            }
            Format::Ollama => r["message"]["tool_calls"][0]["function"]["arguments"] = false.into(),
            _ => (),
        }
        let (c, _) = decode(&r, f, "fixture").unwrap();
        assert!(namzu_core::provider::validate_completion(&c).is_err());
    }
}
#[test]
fn native_unknown_and_ambiguous_blocks_are_rejected() {
    for (f, part) in [
        (Format::Messages, json!({"type":"server_tool_use"})),
        (
            Format::Google,
            json!({"text":"x","functionCall":{"name":"read","args":{}}}),
        ),
        (
            Format::Converse,
            json!({"text":"x","toolUse":{"name":"read","toolUseId":"c","input":{}}}),
        ),
    ] {
        let mut v = reply(f, false);
        match f {
            Format::Messages => v["content"] = json!([part]),
            Format::Google => v["candidates"][0]["content"]["parts"] = json!([part]),
            _ => v["output"]["message"]["content"] = json!([part]),
        };
        assert!(decode(&v, f, "fixture").is_err());
    }
}
#[test]
fn native_usage_normalizes_thoughts_and_cache_without_assuming_missing_zero() {
    let v = json!({"usage":{"input_tokens":3,"cache_read_input_tokens":100,"cache_creation_input_tokens":7,"output_tokens":2}});
    let u = usage(&v, Format::Messages).unwrap().usage.unwrap();
    assert_eq!(u.input_tokens, Some(110));
    assert_eq!(u.total_tokens, Some(112));
    assert_eq!(u.cached_input_tokens, Some(100));
    let v = json!({"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":2,"thoughtsTokenCount":3,"totalTokenCount":15}});
    let u = usage(&v, Format::Google).unwrap().usage.unwrap();
    assert_eq!(u.output_tokens, Some(5));
    assert_eq!(u.reasoning_tokens, Some(3));
    for f in [Format::Messages, Format::Google, Format::Converse] {
        assert!(usage(&json!({}), f).unwrap().usage.is_none());
    }
    assert!(usage(&json!({}), Format::Ollama).unwrap().usage.is_none());
}
#[test]
fn native_usage_negative_fractional_overflow_and_inconsistent_totals_rejected() {
    for n in [json!(-1), json!(0.5), json!("3")] {
        assert!(usage(&json!({"usage":{"input_tokens":n}}), Format::Messages).is_err());
    }
    assert!(
        usage(
            &json!({"usage":{"input_tokens":u64::MAX,"cache_read_input_tokens":1}}),
            Format::Messages
        )
        .is_err()
    );
    assert!(usage(&json!({"usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":2,"totalTokenCount":2}}),Format::Google).is_err());
    let u=usage(&json!({"usage":{"inputTokens":10,"outputTokens":2,"totalTokens":12,"cacheReadInputTokens":900}}),Format::Converse).unwrap().usage.unwrap();
    assert_eq!(u.input_tokens, Some(910));
    assert_eq!(u.total_tokens, Some(912));
}
#[test]
fn native_images_stay_user_data_and_consecutive_roles_merge() {
    let image=namzu_core::ToolImage{width:2,height:2,png_base64:"iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEklEQVR4nGP4z8DAAMIM/4EAAB/uBfsL2WiLAAAAAElFTkSuQmCC".into()};
    image.png().unwrap();
    for service in [
        Service::Anthropic,
        Service::Google,
        Service::Bedrock,
        Service::Ollama,
    ] {
        let p = provider(service, "http://127.0.0.1:1", "fixture");
        let mut m = messages();
        m.push(Message::ToolImage {
            tool_call_id: "image".into(),
            image: image.clone(),
        });
        let body = p.native_body(&m, &[]).unwrap();
        assert!(body.to_string().contains(&image.png_base64));
        assert!(body.to_string().contains("Untrusted image"));
        if service != Service::Ollama {
            let key = if service == Service::Google {
                "contents"
            } else {
                "messages"
            };
            assert_eq!(body[key].as_array().unwrap().len(), 1);
        }
    }
}
#[test]
fn native_error_and_cancel_leave_no_stale_state_or_usage() {
    let (url, worker) = serve(vec![
        Box::new(|_, _, _| (200, reply(Format::Messages, false).to_string())),
        Box::new(|_, _, _| (200, reply(Format::Messages, false).to_string())),
    ]);
    let mut p =
        provider(Service::Anthropic, &url, "fixture").with_pricing(namzu_core::provider::Pricing {
            input_nano_usd_per_token: 2,
            output_nano_usd_per_token: 3,
            cached_input_nano_usd_per_token: None,
        });
    p.complete(&messages(), &[], &ctx()).unwrap();
    assert_eq!(
        p.metadata.as_ref().unwrap().cost.as_ref().unwrap().nano_usd,
        12
    );
    struct Gone;
    impl EventSink for Gone {
        fn emit(&mut self, _: Event) -> Result<(), AgentError> {
            Err(AgentError::Cancelled)
        }
    }
    assert!(matches!(
        p.complete_stream(&messages(), &[], &ctx(), &mut Gone),
        Err(AgentError::Cancelled)
    ));
    assert!(p.take_state().is_none());
    assert!(p.take_metadata().is_none());
    worker.join().unwrap();
    let (url, worker) = serve(vec![Box::new(|_, _, _| {
        (
            200,
            json!({"error":{"code":429,"message":"PRIVATE"}}).to_string(),
        )
    })]);
    let mut p = provider(Service::Google, &url, "fixture");
    let e = p
        .complete_stream(&messages(), &[], &ctx(), &mut NoopEvents)
        .unwrap_err();
    assert!(matches!(e, AgentError::Remote(_)));
    assert!(!e.to_string().contains("PRIVATE"));
    worker.join().unwrap();
}
#[test]
fn unsupported_native_stream_effort_wrong_formats_and_aws_options_fail_at_construction() {
    for service in [
        Service::Anthropic,
        Service::Google,
        Service::Bedrock,
        Service::Ollama,
    ] {
        for options in [
            Options {
                stream: true,
                ..Default::default()
            },
            Options {
                reasoning_effort: Some("low".into()),
                ..Default::default()
            },
            Options {
                format: Format::Chat,
                ..Default::default()
            },
        ] {
            assert!(
                OpenAiCompatible::for_service(
                    service,
                    "http://127.0.0.1:1",
                    "fixture".into(),
                    "key",
                    options
                )
                .is_err()
            );
        }
    }
    assert!(
        Options {
            thinking: Some("enabled".into()),
            ..Default::default()
        }
        .resolved(Service::Anthropic, "fixture")
        .is_err()
    );
    assert!(
        Options {
            bedrock_auth: Some("sigv4".into()),
            ..Default::default()
        }
        .resolved(Service::Bedrock, "fixture")
        .is_err()
    );
    assert!(
        Options {
            reasoning_effort: Some("low".into()),
            ..Default::default()
        }
        .resolved(Service::Deepseek, "fixture")
        .is_err()
    );
}
#[test]
fn local_profiles_have_no_implicit_fake_authorization_and_deepseek_thinking_is_explicit() {
    let (url, worker) = serve(vec![Box::new(|path, headers, body| {
        assert_eq!(path, "/chat/completions");
        assert!(!headers.to_ascii_lowercase().contains("authorization:"));
        assert!(body.get("provider").is_none());
        (200,json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"local"}}]}).to_string())
    })]);
    let mut p = provider(Service::Lmstudio, &url, "fixture");
    assert_eq!(p.complete(&messages(), &[], &ctx()).unwrap().text, "local");
    worker.join().unwrap();
    let (url, worker) = serve(vec![Box::new(|_, _, body| {
        assert_eq!(body["thinking"]["type"], "disabled");
        assert!(body.get("reasoning_effort").is_none());
        (200,json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"deepseek"}}]}).to_string())
    })]);
    let mut p = OpenAiCompatible::for_service(
        Service::Deepseek,
        &url,
        "fixture".into(),
        "fixture-key",
        Options {
            thinking: Some("disabled".into()),
            ..Default::default()
        },
    )
    .unwrap();
    p.client = crate::http::Client::builder().no_proxy().build().unwrap();
    assert_eq!(
        p.complete(&messages(), &[], &ctx()).unwrap().text,
        "deepseek"
    );
    worker.join().unwrap();
}
