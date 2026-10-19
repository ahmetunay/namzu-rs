use namzu_core::{provider::*, *};
use namzu_provider::OpenAiCompatible;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
fn ctx() -> TurnContext {
    TurnContext::new(Duration::from_secs(10), Cancellation::default()).unwrap()
}
fn fixture(
    replies: Vec<(u16, String, String)>,
) -> (String, Arc<AtomicUsize>, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let worker = std::thread::spawn(move || {
        for (status, headers, body) in replies {
            let until = Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < until);
                        std::thread::yield_now();
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut header = vec![];
            let mut byte = [0];
            while !header.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                header.push(byte[0]);
                assert!(header.len() < 16384);
            }
            let header = String::from_utf8(header).unwrap();
            assert!(header.starts_with("POST /v1/chat/completions"));
            let n: usize = header
                .lines()
                .find_map(|s| {
                    s.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|s| s.trim().parse().unwrap())
                })
                .unwrap();
            let mut input = vec![0; n];
            stream.read_exact(&mut input).unwrap();
            let v: Value = serde_json::from_slice(&input).unwrap();
            assert_eq!(v["stream"], false);
            count.fetch_add(1, Ordering::SeqCst);
            write!(stream,"HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n{headers}\r\n{body}",body.len()).unwrap();
        }
    });
    (format!("http://{address}/v1"), calls, worker)
}
fn reply(usage: Value) -> String {
    json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"fixture"}}],"usage":usage}).to_string()
}
fn messages() -> Vec<Message> {
    vec![Message::User {
        content: "fixture".into(),
    }]
}
#[test]
fn usage_and_estimates_are_one_shot_and_reset_before_failed_attempt() {
    let body = reply(
        json!({"prompt_tokens":100,"completion_tokens":10,"total_tokens":110,"prompt_tokens_details":{"cached_tokens":40},"completion_tokens_details":{"reasoning_tokens":5}}),
    );
    let (url, calls, worker) = fixture(vec![
        (200, String::new(), body),
        (
            429,
            "Retry-After: 7\r\n".into(),
            "private-error-body".into(),
        ),
    ]);
    let mut provider = OpenAiCompatible::new(&url, "fixture".into(), "fixture-only")
        .unwrap()
        .with_pricing(Pricing {
            input_nano_usd_per_token: 2000,
            output_nano_usd_per_token: 8000,
            cached_input_nano_usd_per_token: Some(500),
        });
    assert_eq!(provider.info().capabilities.streaming, Streaming::Buffered);
    assert_eq!(
        provider.complete(&messages(), &[], &ctx()).unwrap().text,
        "fixture"
    );
    let metadata = provider.take_metadata().unwrap();
    assert_eq!(metadata.usage.unwrap().cached_input_tokens, Some(40));
    assert_eq!(metadata.cost.unwrap().nano_usd, 220000);
    assert!(provider.take_metadata().is_none());
    let error = provider.complete(&messages(), &[], &ctx()).unwrap_err();
    assert_eq!(
        error.provider_failure().unwrap().kind,
        FailureKind::RateLimited
    );
    assert_eq!(
        error.provider_failure().unwrap().retry_after_seconds,
        Some(7)
    );
    assert!(!error.to_string().contains("private"));
    assert!(provider.take_metadata().is_none());
    worker.join().unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
#[test]
fn absent_usage_and_unknown_cost_never_become_zero_usage_or_free() {
    let (url, _, worker) = fixture(vec![(200, String::new(), reply(Value::Null))]);
    let mut p = OpenAiCompatible::new(&url, "fixture".into(), "fixture-only").unwrap();
    p.complete(&messages(), &[], &ctx()).unwrap();
    assert_eq!(p.take_metadata(), Some(ResponseMetadata::default()));
    worker.join().unwrap();
}
#[test]
fn malformed_usage_rejects_completed_response_and_clears_previous_metadata() {
    let (url, _, worker) = fixture(vec![
        (200, String::new(), reply(json!({"prompt_tokens":1}))),
        (200, String::new(), reply(json!({"prompt_tokens":-1}))),
    ]);
    let mut p = OpenAiCompatible::new(&url, "fixture".into(), "fixture-only").unwrap();
    p.complete(&messages(), &[], &ctx()).unwrap();
    assert!(p.complete(&messages(), &[], &ctx()).is_err());
    assert!(p.take_metadata().is_none());
    worker.join().unwrap();
}
#[test]
fn shared_request_validation_prevents_http_before_bad_schema() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let mut p = OpenAiCompatible::new(&url, "fixture".into(), "fixture-only").unwrap();
    assert!(
        p.complete(
            &messages(),
            &[ToolDefinition {
                name: "bad\nname".into(),
                description: "fixture".into(),
                parameters: json!({})
            }],
            &ctx()
        )
        .is_err()
    );
    assert!(listener.accept().is_err());
}
#[test]
fn error_http_classes_are_redacted_without_automatic_retry() {
    for status in [401, 403, 404, 400, 503] {
        let (url, calls, worker) = fixture(vec![(
            status,
            "Retry-After: private\r\n".into(),
            "private-payload".into(),
        )]);
        let mut p = OpenAiCompatible::new(&url, "fixture".into(), "fixture-only").unwrap();
        let e = p.complete(&messages(), &[], &ctx()).unwrap_err();
        assert_eq!(e.provider_failure().unwrap().http_status, Some(status));
        assert!(e.provider_failure().unwrap().retry_after_seconds.is_none());
        assert!(!format!("{e:?}").contains("private"));
        worker.join().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
