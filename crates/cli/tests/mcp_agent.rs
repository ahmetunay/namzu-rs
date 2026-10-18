use namzu_mcp::{agent::alias, discovery::Kind};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::{Command, Output},
    thread,
    time::{Duration, Instant},
};
fn run(state: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_namzu-rs"))
        .arg(args[0])
        .args(["--state-dir", state.to_str().unwrap()])
        .args(&args[1..])
        .env("NAMZU_AGENT_FIXTURE_KEY", "fixture-only")
        .output()
        .unwrap()
}
fn value(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&out.stderr)))
}
fn import(root: &Path, mode: &str) {
    let spec = root.join("spec.json");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../mcp/tests/fixtures/discovery_server.py")
        .canonicalize()
        .unwrap();
    std::fs::write(&spec,json!({"transport":{"type":"stdio","program":"/usr/bin/python3","args":[fixture,mode],"cwd":root},"policy":{"require_approval":true}}).to_string()).unwrap();
    assert!(
        run(
            &root.join("state"),
            &["mcp", "add", "fixture", "--spec", spec.to_str().unwrap()]
        )
        .status
        .success()
    );
}
fn requests(root: &Path) -> Vec<Value> {
    std::fs::read_to_string(root.join("requests.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect()
}
fn model(
    count: usize,
    mut callback: impl FnMut(Value, usize) -> Value + Send + 'static,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let worker = thread::spawn(move || {
        for n in 0..count {
            let until = Instant::now() + Duration::from_secs(10);
            let mut socket = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < until);
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut headers = vec![];
            let mut b = [0];
            while !headers.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut b).unwrap();
                headers.push(b[0]);
                assert!(headers.len() < 16384);
            }
            let headers = String::from_utf8(headers).unwrap();
            let size = headers
                .lines()
                .find_map(|s| {
                    s.to_lowercase()
                        .strip_prefix("content-length: ")
                        .map(|n| n.parse::<usize>().unwrap())
                })
                .unwrap();
            let mut body = vec![0; size];
            socket.read_exact(&mut body).unwrap();
            let response = callback(serde_json::from_slice(&body).unwrap(), n).to_string();
            write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",response.len(),response).unwrap();
        }
    });
    (url, worker)
}
fn tool(name: &str, args: Value) -> Value {
    json!({"id":"mcp-call","type":"function","function":{"name":name,"arguments":args.to_string()}})
}
fn response(calls: Vec<Value>) -> Value {
    json!({"choices":[{"finish_reason":if calls.is_empty(){"stop"}else{"tool_calls"},"message":{"role":"assistant","content":if calls.is_empty(){"MCP_DONE"}else{""},"tool_calls":calls}}]})
}
#[test]
fn exec_model_mcp_journal_and_resume_pin_work_end_to_end() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    import(root.path(), "normal");
    let name = alias("fixture", Kind::Tools, "read");
    let expected = name.clone();
    let (url, worker) = model(2, move |body, n| {
        if n == 0 {
            assert!(
                body["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|d| d["function"]["name"] == expected)
            );
            response(vec![tool(&expected, json!({"path":"a"}))])
        } else {
            let tool = body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["role"] == "tool")
                .unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(serde_json::from_str::<Value>(tool["content"].as_str().unwrap()).unwrap()["content"].as_str().unwrap()).unwrap()["structuredContent"]["answer"],
                42
            );
            response(vec![])
        }
    });
    let result = run(
        &state,
        &[
            "exec",
            "test",
            "--cwd",
            root.path().to_str().unwrap(),
            "--model",
            "fixture",
            "--base-url",
            &url,
            "--api-key-env",
            "NAMZU_AGENT_FIXTURE_KEY",
            "--mcp-server",
            "fixture",
            "--mcp-approve",
            "--json",
        ],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    worker.join().unwrap();
    let result = value(&result);
    let id = result["session_id"].as_str().unwrap();
    assert_eq!(result["stats"]["tool_calls"], 1);
    let session: Value =
        serde_json::from_slice(&std::fs::read(state.join(format!("{id}.json"))).unwrap()).unwrap();
    assert_eq!(session["status"], "complete");
    assert!(session["pending_call"].is_null());
    assert_eq!(
        requests(root.path())
            .iter()
            .filter(|r| r["method"] == "tools/call")
            .count(),
        1
    );
    assert!(run(&state, &["mcp", "disable", "fixture"]).status.success());
    let resume = run(
        &state,
        &[
            "exec",
            "again",
            "--cwd",
            root.path().to_str().unwrap(),
            "--model",
            "fixture",
            "--base-url",
            &url,
            "--api-key-env",
            "NAMZU_AGENT_FIXTURE_KEY",
            "--mcp-server",
            "fixture",
            "--mcp-approve",
            "--resume",
            id,
            "--json",
        ],
    );
    assert!(!resume.status.success());
    assert_eq!(
        requests(root.path())
            .iter()
            .filter(|r| r["method"] == "tools/call")
            .count(),
        1
    );
}
#[test]
fn exec_no_approval_does_not_spawn_mcp_or_contact_model() {
    let root = tempfile::tempdir().unwrap();
    import(root.path(), "normal");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let result = run(
        &root.path().join("state"),
        &[
            "exec",
            "test",
            "--cwd",
            root.path().to_str().unwrap(),
            "--model",
            "fixture",
            "--base-url",
            &url,
            "--api-key-env",
            "NAMZU_AGENT_FIXTURE_KEY",
            "--mcp-server",
            "fixture",
            "--json",
        ],
    );
    assert!(!result.status.success());
    assert!(!root.path().join("requests.jsonl").exists());
    assert!(matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
}
#[test]
fn invalid_argument_in_batch_prevents_first_local_write() {
    let root = tempfile::tempdir().unwrap();
    import(root.path(), "normal");
    let name = alias("fixture", Kind::Tools, "read");
    let (url, worker) = model(1, move |_, _| {
        let mut local = tool(
            "write_file",
            json!({"path":"must-not-exist","content":"bad"}),
        );
        local["id"] = "local-call".into();
        response(vec![local, tool(&name, json!({"path":42}))])
    });
    let result = run(
        &root.path().join("state"),
        &[
            "exec",
            "test",
            "--cwd",
            root.path().to_str().unwrap(),
            "--model",
            "fixture",
            "--base-url",
            &url,
            "--api-key-env",
            "NAMZU_AGENT_FIXTURE_KEY",
            "--mcp-server",
            "fixture",
            "--mcp-approve",
            "--allow-write",
            "--json",
        ],
    );
    assert!(!result.status.success());
    worker.join().unwrap();
    assert!(!root.path().join("must-not-exist").exists());
    assert!(
        !requests(root.path())
            .iter()
            .any(|r| r["method"] == "tools/call")
    );
}
#[test]
fn output_schema_failure_keeps_pending_journal_and_cannot_resume() {
    let root = tempfile::tempdir().unwrap();
    import(root.path(), "agent_bad_output");
    let name = alias("fixture", Kind::Tools, "read");
    let (url, worker) = model(1, move |_, _| {
        response(vec![tool(&name, json!({"path":"a"}))])
    });
    let state = root.path().join("state");
    let result = run(
        &state,
        &[
            "exec",
            "test",
            "--cwd",
            root.path().to_str().unwrap(),
            "--model",
            "fixture",
            "--base-url",
            &url,
            "--api-key-env",
            "NAMZU_AGENT_FIXTURE_KEY",
            "--mcp-server",
            "fixture",
            "--mcp-approve",
            "--json",
        ],
    );
    assert!(!result.status.success());
    worker.join().unwrap();
    let result = value(&result);
    let id = result["session_id"].as_str().unwrap();
    let session: Value =
        serde_json::from_slice(&std::fs::read(state.join(format!("{id}.json"))).unwrap()).unwrap();
    assert_eq!(session["status"], "failed");
    assert_eq!(session["pending_call"], "mcp-call");
    assert_eq!(
        requests(root.path())
            .iter()
            .filter(|r| r["method"] == "tools/call")
            .count(),
        1
    );
}
#[test]
fn pal_pins_mcp_selection_and_preserves_exact_tool_ceiling() {
    let root = tempfile::tempdir().unwrap();
    import(root.path(), "normal");
    let name = alias("fixture", Kind::Tools, "read");
    let expected = name.clone();
    let (url, worker) = model(2, move |body, n| {
        if n == 0 {
            assert_eq!(body["tools"].as_array().unwrap().len(), 1);
            assert_eq!(body["tools"][0]["function"]["name"], expected);
            response(vec![tool(&expected, json!({"path":"a"}))])
        } else {
            response(vec![])
        }
    });
    let state = root.path().join("state");
    let created = run(
        &state,
        &[
            "pal",
            "--model",
            "fixture",
            "--base-url",
            &url,
            "--api-key-env",
            "NAMZU_AGENT_FIXTURE_KEY",
            "--mcp-server",
            "fixture",
            "--tool-allow",
            &name,
            "create",
            "MCP Pal",
        ],
    );
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stdout)
    );
    let p = value(&created);
    let id = p["result"]["id"].as_str().unwrap();
    let result = run(&state, &["pal", "--mcp-approve", "ask", id, "test"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    worker.join().unwrap();
    assert_eq!(
        requests(root.path())
            .iter()
            .filter(|r| r["method"] == "tools/call")
            .count(),
        1
    );
    assert!(run(&state, &["mcp", "disable", "fixture"]).status.success());
    let result = run(&state, &["pal", "--mcp-approve", "ask", id, "again"]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stdout).contains("MCP configuration changed"));
}
#[test]
fn pal_does_not_gain_unlisted_mcp_tools_from_saved_server_selection() {
    let root = tempfile::tempdir().unwrap();
    import(root.path(), "normal");
    let name = alias("fixture", Kind::Tools, "read");
    let (url, worker) = model(2, move |body, n| {
        assert!(
            !body["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["function"]["name"].as_str().unwrap().starts_with("mcp_"))
        );
        if n == 0 {
            response(vec![tool(&name, json!({"path":"a"}))])
        } else {
            response(vec![])
        }
    });
    let state = root.path().join("state");
    let created = run(
        &state,
        &[
            "pal",
            "--model",
            "fixture",
            "--base-url",
            &url,
            "--api-key-env",
            "NAMZU_AGENT_FIXTURE_KEY",
            "--mcp-server",
            "fixture",
            "create",
            "Limited Pal",
        ],
    );
    assert!(created.status.success());
    let p = value(&created);
    let id = p["result"]["id"].as_str().unwrap();
    let result = run(&state, &["pal", "--mcp-approve", "ask", id, "test"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    worker.join().unwrap();
    assert!(requests(root.path()).is_empty());
}
