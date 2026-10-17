use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    time::Duration,
};

#[test]
fn config_init_is_private_and_does_not_overwrite() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state");
    let run = |action: &str| {
        Command::new(env!("CARGO_BIN_EXE_namzu-rs"))
            .args(["config", "--state-dir", path.to_str().unwrap(), action])
            .output()
            .unwrap()
    };
    assert!(run("init").status.success());
    assert!(!run("init").status.success());
    let output = run("show");
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["timeout"], 300);
    assert_eq!(value["api_key_env"], "OPENAI_API_KEY");
    assert!(value.get("api_key").is_none());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path.join("config.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn selected_config_rejects_unknown_fields_and_cannot_grant_shell() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state");
    let config = root.path().join("selected.json");
    std::fs::write(
        &config,
        json!({"allow_shell":true,"api_key":"do-not-echo"}).to_string(),
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_namzu-rs"))
        .args([
            "exec",
            "test",
            "--model",
            "fixture",
            "--config",
            config.to_str().unwrap(),
            "--state-dir",
            path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!String::from_utf8_lossy(&result.stdout).contains("do-not-echo"));
}

#[test]
fn real_resume_replays_history_and_does_not_restore_write_grants() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for turn in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
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
            let length = header
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                        .map(|(_, v)| v.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            let mut bytes = vec![0; length];
            stream.read_exact(&mut bytes).unwrap();
            let request: Value = serde_json::from_slice(&bytes).unwrap();
            if turn == 0 {
                assert_eq!(request["messages"].as_array().unwrap().len(), 1);
            } else {
                assert_eq!(request["messages"].as_array().unwrap().len(), 3);
                assert_eq!(request["messages"][0]["content"], "first");
                assert_eq!(request["messages"][1]["content"], "response-0");
                assert_eq!(request["messages"][2]["content"], "second");
                assert!(
                    request["tools"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|tool| tool["function"]["name"] != "write_file"
                            && tool["function"]["name"] != "apply_patch")
                );
            }
            let body=json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":format!("response-{turn}")}}]}).to_string();
            write!(stream,"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",body.len(),body).unwrap();
        }
    });
    let run = |prompt: &str, resume: Option<&str>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_namzu-rs"));
        command
            .args([
                "exec",
                prompt,
                "--model",
                "fixture",
                "--base-url",
                &format!("http://{address}/v1"),
                "--api-key-env",
                "FIXTURE_KEY",
                "--cwd",
                root.path().to_str().unwrap(),
                "--state-dir",
                state.to_str().unwrap(),
                "--json",
                "--timeout",
                "10",
            ])
            .env("FIXTURE_KEY", "local-fixture")
            .env("NO_PROXY", "127.0.0.1")
            .env("no_proxy", "127.0.0.1");
        if let Some(id) = resume {
            command.args(["--resume", id]);
        } else {
            command.arg("--allow-write");
        }
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
        serde_json::from_slice::<Value>(&result.stdout).unwrap()
    };
    let first = run("first", None);
    let id = first["session_id"].as_str().unwrap();
    let second = run("second", Some(id));
    assert_eq!(second["session_id"], id);
    server.join().unwrap();
    let store = namzu_session::PrivateStore::open(&state).unwrap();
    let session = namzu_session::Session::load(&store, id).unwrap();
    assert_eq!(session.messages.len(), 4);
    assert_eq!(session.status, namzu_session::SessionStatus::Complete);
}
