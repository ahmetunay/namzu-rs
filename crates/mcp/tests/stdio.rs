#![cfg(target_os = "linux")]
use namzu_core::{Cancellation, TurnContext};
use namzu_mcp::{
    Incoming, Limits, Phase,
    stdio::{Client, Error, Spec},
    types::Implementation,
};
use serde_json::{Map, Value, json};
use std::{
    collections::BTreeMap,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::Path,
    time::Duration,
};

fn ctx() -> TurnContext {
    TurnContext::new(Duration::from_secs(20), Cancellation::default()).unwrap()
}
fn spec(root: &Path, mode: &str) -> Spec {
    let mut spec = Spec::new("/usr/bin/python3", root.canonicalize().unwrap());
    spec.args = vec![
        format!(
            "{}/tests/fixtures/stdio_server.py",
            env!("CARGO_MANIFEST_DIR")
        ),
        mode.into(),
    ];
    spec
}
fn connect(root: &Path, mode: &str, lifetime: &TurnContext) -> Client {
    Client::connect(
        spec(root, mode),
        Implementation::new("namzu-fixture", "1"),
        Limits::default(),
        lifetime,
    )
    .unwrap()
}
fn params(v: Value) -> Option<Map<String, Value>> {
    Some(v.as_object().unwrap().clone())
}
fn pidfd(pid: u32) -> OwnedFd {
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    assert!(fd >= 0);
    unsafe { OwnedFd::from_raw_fd(fd as i32) }
}
fn dead(fd: &OwnedFd) {
    let mut p = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    assert_eq!(unsafe { libc::poll(&mut p, 1, 1000) }, 1);
}
fn started(client: &mut Client, c: &TurnContext) -> u32 {
    let Incoming::Notification {
        method,
        params: Some(p),
    } = client.next(c).unwrap()
    else {
        panic!("missing readiness")
    };
    assert_eq!(method, "notifications/fixture_started");
    p["pid"].as_u64().unwrap() as u32
}
#[test]
fn real_stdio_initialization_call_and_graceful_eof_shutdown() {
    let root = tempfile::tempdir().unwrap();
    let life = ctx();
    let mut c = connect(root.path(), "normal", &life);
    assert_eq!(c.phase(), Phase::Ready);
    let result = c
        .call("tools/call", params(json!({"name":"fixture"})), &ctx())
        .unwrap()
        .unwrap();
    assert_eq!(result["answer"], 42);
    let fd = pidfd(c.pid());
    let report = c.close();
    assert_eq!(report.exit_code, Some(0));
    assert!(!report.forced);
    assert!(report.abandoned.is_empty());
    assert_eq!(c.close(), report);
    dead(&fd);
    assert!(root.path().join("graceful").exists());
    assert!(matches!(c.send("ping", None, &ctx()), Err(Error::Closed)));
}
#[test]
fn environment_and_cwd_are_explicit_and_arguments_are_not_interpreted_by_a_shell() {
    let root = tempfile::tempdir().unwrap();
    let mut s = spec(root.path(), "environment");
    s.env = BTreeMap::from([("MCP_FIXTURE_VALUE".into(), "fixture-explicit".into())]);
    s.args
        .push("; touch INJECTED; $(touch ALSO_INJECTED)".into());
    let mut c =
        Client::connect(s, Implementation::new("n", "1"), Limits::default(), &ctx()).unwrap();
    let result = c.call("ping", None, &ctx()).unwrap().unwrap();
    assert_eq!(result["cwd"], root.path().to_str().unwrap());
    assert_eq!(result["explicit"], "fixture-explicit");
    assert_eq!(result["forbidden_present"], false);
    assert_eq!(
        result["literal_args"][0],
        "; touch INJECTED; $(touch ALSO_INJECTED)"
    );
    assert!(!root.path().join("INJECTED").exists());
    assert!(!root.path().join("ALSO_INJECTED").exists());
}
#[test]
fn duplex_large_stdin_stdout_and_stderr_do_not_deadlock_or_log_diagnostics() {
    let root = tempfile::tempdir().unwrap();
    let mut c = connect(root.path(), "duplex", &ctx());
    let result = c
        .call(
            "tools/call",
            params(json!({"padding":"x".repeat(200000)})),
            &ctx(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(result["bytes"], 200000);
    c.close();
    let tail = c.stderr_tail();
    assert_eq!(tail.len(), 16384);
    assert!(tail.iter().all(|b| *b == b'L'));
    let report = c.close_report().unwrap();
    assert_eq!(report.stderr_bytes, 200000);
    assert!(report.stderr_truncated);
}
#[test]
fn pipelined_replies_remain_correlated_when_peer_reverses_them() {
    let root = tempfile::tempdir().unwrap();
    let mut c = connect(root.path(), "pipeline", &ctx());
    let first = c
        .send("tools/call", params(json!({"marker":"first"})), &ctx())
        .unwrap();
    let second = c
        .send("tools/call", params(json!({"marker":"second"})), &ctx())
        .unwrap();
    assert!(matches!(c.call("ping", None, &ctx()), Err(Error::Busy)));
    for (id, value) in [(second, "second"), (first, "first")] {
        let Incoming::Reply {
            id: actual,
            result: Ok(result),
            ..
        } = c.next(&ctx()).unwrap()
        else {
            panic!()
        };
        assert_eq!(actual, id);
        assert_eq!(result["marker"], value);
    }
}
#[test]
fn explicit_cancel_before_first_write_does_not_dispatch_the_queued_tool() {
    let root = tempfile::tempdir().unwrap();
    let mut c = connect(root.path(), "normal", &ctx());
    let id = c.send("tools/call", None, &ctx()).unwrap();
    assert!(c.cancel(&id).unwrap());
    assert!(!root.path().join("effects").exists());
    assert_eq!(c.close_report().unwrap().abandoned[0].id, id);
}
#[test]
fn cancellation_after_real_effect_stops_leader_and_inherited_pipe_descendant() {
    let root = tempfile::tempdir().unwrap();
    let mut c = connect(root.path(), "ignore", &ctx());
    let leader = pidfd(c.pid());
    let id = c.send("tools/call", None, &ctx()).unwrap();
    let child = pidfd(started(&mut c, &ctx()));
    assert!(c.cancel(&id).unwrap());
    dead(&leader);
    dead(&child);
    assert!(c.close_report().unwrap().forced);
    assert_eq!(c.close_report().unwrap().abandoned[0].id, id);
    assert_eq!(
        std::fs::read_to_string(root.path().join("effects")).unwrap(),
        "1"
    );
}
#[test]
fn drop_stops_a_live_server_and_its_group() {
    let root = tempfile::tempdir().unwrap();
    let mut c = connect(root.path(), "wait", &ctx());
    let leader = pidfd(c.pid());
    c.send("tools/call", None, &ctx()).unwrap();
    let child = pidfd(started(&mut c, &ctx()));
    drop(c);
    dead(&leader);
    dead(&child);
}
#[test]
fn cancelled_request_context_closes_process_and_preserves_uncertain_binding() {
    let root = tempfile::tempdir().unwrap();
    let mut c = connect(root.path(), "wait", &ctx());
    let request = ctx();
    let id = c.send("tools/call", None, &request).unwrap();
    let child = pidfd(started(&mut c, &ctx()));
    request.cancellation().cancel();
    assert!(matches!(
        c.next(&ctx()),
        Err(Error::Context(namzu_core::AgentError::Cancelled))
    ));
    dead(&child);
    assert_eq!(c.close_report().unwrap().abandoned[0].id, id);
}
#[test]
fn lifetime_cancellation_fences_every_pending_request() {
    let root = tempfile::tempdir().unwrap();
    let life = ctx();
    let mut c = connect(root.path(), "wait", &life);
    let id = c.send("tools/call", None, &ctx()).unwrap();
    let child = pidfd(started(&mut c, &ctx()));
    life.cancellation().cancel();
    assert!(matches!(
        c.next(&ctx()),
        Err(Error::Context(namzu_core::AgentError::Cancelled))
    ));
    dead(&child);
    assert_eq!(c.close_report().unwrap().abandoned[0].id, id);
}
#[test]
fn deadline_after_acknowledged_start_closes_remote_work_without_replay() {
    let root = tempfile::tempdir().unwrap();
    let mut c = connect(root.path(), "wait", &ctx());
    let request = TurnContext::new(Duration::from_secs(1), Cancellation::default()).unwrap();
    let id = c.send("tools/call", None, &request).unwrap();
    let child = pidfd(started(&mut c, &ctx()));
    while request.remaining().is_ok() {
        std::thread::yield_now();
    }
    assert!(matches!(
        c.next(&ctx()),
        Err(Error::Context(namzu_core::AgentError::Timeout))
    ));
    dead(&child);
    assert_eq!(c.close_report().unwrap().abandoned[0].id, id);
    assert_eq!(
        std::fs::read_to_string(root.path().join("effects")).unwrap(),
        "1"
    );
}
#[test]
fn leader_exit_with_descendant_holding_pipes_does_not_wait_for_eof() {
    let root = tempfile::tempdir().unwrap();
    let mut c = connect(root.path(), "early_exit", &ctx());
    c.send("tools/call", None, &ctx()).unwrap();
    let child = pidfd(started(&mut c, &ctx()));
    assert!(matches!(c.next(&ctx()), Err(Error::Eof)));
    dead(&child);
}
#[test]
fn final_complete_reply_is_not_discarded_when_peer_exits_immediately() {
    let root = tempfile::tempdir().unwrap();
    let mut c = connect(root.path(), "exit_reply", &ctx());
    let result = c.call("tools/call", None, &ctx()).unwrap().unwrap();
    assert_eq!(result["answer"], 42);
    c.close();
}
#[test]
fn unknown_response_id_closes_and_retains_uncertain_effect_without_retry() {
    let root = tempfile::tempdir().unwrap();
    let mut c = connect(root.path(), "bad_id", &ctx());
    let id = c.send("tools/call", None, &ctx()).unwrap();
    assert!(matches!(
        c.next(&ctx()),
        Err(Error::Protocol(namzu_mcp::Error::ResponseId))
    ));
    assert_eq!(c.close_report().unwrap().abandoned[0].id, id);
    assert_eq!(
        std::fs::read_to_string(root.path().join("effects")).unwrap(),
        "1"
    );
}
#[test]
fn protocol_rpc_error_and_tool_failure_remain_distinct_and_do_not_kill_healthy_transport() {
    for mode in ["rpc_error", "tool_error"] {
        let root = tempfile::tempdir().unwrap();
        let mut c = connect(root.path(), mode, &ctx());
        let result = c.call("tools/call", None, &ctx()).unwrap();
        if mode == "rpc_error" {
            assert_eq!(result.unwrap_err().code, -32602);
        } else {
            assert_eq!(result.unwrap()["isError"], true);
        }
        assert!(c.close_report().is_none());
    }
}
#[test]
fn server_initiated_ping_and_unsupported_request_are_answered_without_host_effects() {
    let root = tempfile::tempdir().unwrap();
    let mut c = connect(root.path(), "server_request", &ctx());
    let result = c.call("tools/call", None, &ctx()).unwrap().unwrap();
    assert_eq!(result["answered_server_requests"], true);
}
#[test]
fn broken_protocol_oversize_stderr_and_truncated_startup_fail_with_redacted_errors() {
    for mode in [
        "version",
        "stdout_noise",
        "oversize",
        "stderr_flood",
        "truncated",
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut s = spec(root.path(), mode);
        s.stderr_limit = 128;
        s.stderr_capture = 64;
        let limits = Limits {
            message_bytes: 4096,
            string_bytes: 2048,
            ..Limits::default()
        };
        let error = match Client::connect(s, Implementation::new("n", "1"), limits, &ctx()) {
            Ok(_) => panic!("unexpected success {mode}"),
            Err(e) => e,
        };
        assert!(!error.to_string().contains("FIXTURE_PRIVATE"));
        let pid = std::fs::read_to_string(root.path().join("leader.pid"))
            .unwrap()
            .parse::<i32>()
            .unwrap();
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    }
}
#[test]
fn invalid_configuration_and_pre_cancelled_context_do_not_start_process() {
    let root = tempfile::tempdir().unwrap();
    let request = ctx();
    request.cancellation().cancel();
    assert!(matches!(
        Client::connect(
            spec(root.path(), "normal"),
            Implementation::new("n", "1"),
            Limits::default(),
            &request
        ),
        Err(Error::Context(_))
    ));
    assert!(!root.path().join("leader.pid").exists());
    let mut s = spec(root.path(), "normal");
    s.program = "python3".into();
    assert!(matches!(
        Client::connect(s, Implementation::new("n", "1"), Limits::default(), &ctx()),
        Err(Error::Configuration)
    ));
    let mut s = spec(root.path(), "normal");
    s.env.insert("BAD=NAME".into(), "x".into());
    assert!(matches!(
        Client::connect(s, Implementation::new("n", "1"), Limits::default(), &ctx()),
        Err(Error::Configuration)
    ));
    assert!(!root.path().join("leader.pid").exists());
}

#[test]
fn cancellation_during_partial_write_does_not_finish_dispatching_the_tool() {
    let root = tempfile::tempdir().unwrap();
    let mut c = connect(root.path(), "partial_read", &ctx());
    let leader = pidfd(c.pid());
    let id = c
        .send(
            "tools/call",
            params(json!({"padding":"x".repeat(200000)})),
            &ctx(),
        )
        .unwrap();
    let Incoming::Notification {
        method,
        params: Some(p),
    } = c.next(&ctx()).unwrap()
    else {
        panic!()
    };
    assert_eq!(method, "notifications/partial_read");
    assert_eq!(p["bytes"], 64);
    c.cancel(&id).unwrap();
    dead(&leader);
    assert!(!root.path().join("effects").exists());
    assert_eq!(c.close_report().unwrap().abandoned[0].id, id);
}
#[test]
fn initialization_wait_is_cancelled_after_peer_readiness_and_cleans_the_child() {
    let root = tempfile::tempdir().unwrap();
    let c = ctx();
    let signal = c.cancellation();
    std::thread::scope(|scope| {
        let path = root.path().join("seen");
        let watcher = scope.spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while !path.exists() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "fixture never read initialize"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
            signal.cancel();
        });
        assert!(matches!(
            Client::connect(
                spec(root.path(), "init_stall"),
                Implementation::new("n", "1"),
                Limits::default(),
                &c
            ),
            Err(Error::Context(namzu_core::AgentError::Cancelled))
        ));
        watcher.join().unwrap();
    });
    let pid = std::fs::read_to_string(root.path().join("leader.pid"))
        .unwrap()
        .parse::<i32>()
        .unwrap();
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
}
#[test]
fn lifetime_stdout_budget_terminates_a_progress_flood_and_retains_pending_id() {
    let root = tempfile::tempdir().unwrap();
    let mut s = spec(root.path(), "stdout_flood");
    s.stdout_limit = 8192;
    let mut c =
        Client::connect(s, Implementation::new("n", "1"), Limits::default(), &ctx()).unwrap();
    let id = c.send("tools/call", None, &ctx()).unwrap();
    loop {
        match c.next(&ctx()) {
            Ok(Incoming::Notification { .. }) => (),
            Err(Error::OutputLimit) => break,
            _ => panic!("unexpected outcome"),
        }
    }
    assert_eq!(c.close_report().unwrap().abandoned[0].id, id);
}
#[test]
fn stdio_host_subprocess_fixture() {
    let Some(root) = std::env::var_os("NAMZU_MCP_HOST_FIXTURE") else {
        return;
    };
    let root = Path::new(&root);
    let mut client = connect(root, "normal", &ctx());
    std::fs::write(root.join("host_ready.tmp"), client.pid().to_string()).unwrap();
    std::fs::rename(root.join("host_ready.tmp"), root.join("host_ready")).unwrap();
    let _ = client.next(&ctx());
}
#[test]
fn linux_parent_death_signal_stops_the_direct_server_when_host_is_killed() {
    struct Host(std::process::Child);
    impl Drop for Host {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let root = tempfile::tempdir().unwrap();
    let mut host = Host(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "stdio_host_subprocess_fixture", "--nocapture"])
            .env("NAMZU_MCP_HOST_FIXTURE", root.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let ready = root.path().join("host_ready");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !ready.exists() {
        assert!(host.0.try_wait().unwrap().is_none(), "host exited early");
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    let server = pidfd(std::fs::read_to_string(ready).unwrap().parse().unwrap());
    host.0.kill().unwrap();
    host.0.wait().unwrap();
    dead(&server);
}
