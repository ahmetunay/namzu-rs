//! Real loopback HTTP through the CLI. Fixture key only; no paid model calls.
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::Command,
    time::Duration,
};

fn request(stream: &mut TcpStream) -> Value {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut header = vec![];
    while !header.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        header.push(byte[0]);
        assert!(header.len() < 16384);
    }
    let header = String::from_utf8(header).unwrap();
    assert!(header.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
    assert!(
        header
            .to_ascii_lowercase()
            .contains("authorization: bearer test-fixture-only\r\n")
    );
    let length = header
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse::<usize>().unwrap())
        })
        .unwrap();
    assert!(length < 1024 * 1024);
    let mut body = vec![0; length];
    stream.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn reply(stream: &mut TcpStream, body: Value) {
    let body = body.to_string();
    write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",body.len(),body).unwrap();
}

#[test]
fn real_cli_replays_tool_result_through_http_provider() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("input.txt"), "12\n").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut first, _) = listener.accept().unwrap();
        let body = request(&mut first);
        assert_eq!(body["model"], "fixture");
        assert_eq!(body["messages"][0]["content"], "Read input.txt");
        assert!(
            body["tools"]
                .as_array()
                .unwrap()
                .iter()
                .all(|tool| tool["function"]["name"] != "write_file")
        );
        reply(
            &mut first,
            json!({"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"id":"read-1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"input.txt\"}"}}]}}]}),
        );
        drop(first);
        let (mut second, _) = listener.accept().unwrap();
        let body = request(&mut second);
        let output = &body["messages"][2];
        assert_eq!(output["role"], "tool");
        assert_eq!(output["tool_call_id"], "read-1");
        let content: Value = serde_json::from_str(output["content"].as_str().unwrap()).unwrap();
        assert_eq!(content, json!({"content":"12\n","is_error":false}));
        reply(
            &mut second,
            json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"12"}}]}),
        );
    });
    let output = Command::new(env!("CARGO_BIN_EXE_namzu-rs"))
        .args([
            "exec",
            "Read input.txt",
            "--cwd",
            root.path().to_str().unwrap(),
            "--model",
            "fixture",
            "--base-url",
            &format!("http://{address}/v1"),
            "--api-key-env",
            "NAMZU_TEST_FIXTURE_KEY",
            "--json",
            "--timeout",
            "10",
        ])
        .env("NAMZU_STATE_DIR", root.path().join("state"))
        .env("NAMZU_TEST_FIXTURE_KEY", "test-fixture-only")
        .env("NO_PROXY", "127.0.0.1")
        .env("no_proxy", "127.0.0.1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    server.join().unwrap();
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["output"], "12");
    assert_eq!(value["stats"], json!({"provider_calls":2,"tool_calls":1}));
}

#[test]
fn provider_error_body_never_leaks_secrets_into_cli_output() {
    let root = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _ = request(&mut stream);
        let body = "test-fixture-only sensitive-upstream-payload";
        write!(
            stream,
            "HTTP/1.1 429 Too Many Requests\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    let output = Command::new(env!("CARGO_BIN_EXE_namzu-rs"))
        .args([
            "exec",
            "test",
            "--model",
            "fixture",
            "--base-url",
            &format!("http://{address}/v1"),
            "--api-key-env",
            "NAMZU_TEST_FIXTURE_KEY",
            "--json",
            "--timeout",
            "10",
        ])
        .env("NAMZU_STATE_DIR", root.path().join("state"))
        .env("NAMZU_TEST_FIXTURE_KEY", "test-fixture-only")
        .env("NO_PROXY", "127.0.0.1")
        .env("no_proxy", "127.0.0.1")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains("test-fixture-only"));
    assert!(!stdout.contains("sensitive-upstream-payload"));
    let value: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["error"], "provider failure: HTTP status 429");
    server.join().unwrap();
}
