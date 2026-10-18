use namzu_core::{Cancellation, TurnContext};
use namzu_mcp::{
    config::{Decision, Document, Error, Policy, Project, Registry},
    manager::{Manager, Outcome},
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path, time::Duration};
fn context() -> TurnContext {
    TurnContext::new(Duration::from_secs(10), Cancellation::default()).unwrap()
}
fn ceiling() -> Policy {
    Policy {
        require_approval: false,
        ..Policy::default()
    }
}
fn doc(server: Value) -> Document {
    Document::parse(json!({"version":1,"default_profile":"default","profiles":{"default":{"servers":{"fixture":server}}}}).to_string().as_bytes()).unwrap()
}
fn http_server() -> Value {
    json!({"transport":{"type":"http","url":"http://127.0.0.1:1/mcp"}})
}
fn params(name: &str) -> Option<serde_json::Map<String, Value>> {
    Some(
        json!({"name":name,"arguments":{}})
            .as_object()
            .unwrap()
            .clone(),
    )
}
fn registry(root: &Path) -> Registry {
    Registry::open(&root.join("state")).unwrap()
}
#[test]
fn private_profiles_are_atomic_and_lock_exclusive() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path());
    r.init().unwrap();
    assert!(r.init().is_err());
    r.replace(doc(http_server())).unwrap();
    let store = namzu_session::PrivateStore::open(root.path().join("state/mcp")).unwrap();
    let lock = store.lock("registry").unwrap();
    assert!(r.create_profile("other").is_err());
    drop(lock);
    r.create_profile("other").unwrap();
    r.default_profile("other").unwrap();
    assert!(r.remove_profile("other").is_err());
    r.remove_profile("default").unwrap();
    assert_eq!(
        r.profiles().unwrap(),
        ("other".into(), vec!["other".to_owned()])
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(root.path().join("state/mcp/profiles.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
#[test]
fn config_rejects_unknown_duplicate_invalid_selectors_and_excess() {
    for value in [
        json!({"version":2,"default_profile":"default","profiles":{}}),
        json!({"version":1,"default_profile":"missing","profiles":{}}),
    ] {
        assert!(Document::parse(value.to_string().as_bytes()).is_err());
    }
    let mut server = http_server();
    server["transport"]["bearer"] = "DO_NOT_ECHO".into();
    assert!(Document::parse(json!({"version":1,"default_profile":"default","profiles":{"default":{"servers":{"f":server}}}}).to_string().as_bytes()).is_err());
    for selector in ["", "9BAD", "HAS-DASH", "NEW\nLINE"] {
        let mut s = http_server();
        s["transport"]["bearer_env"] = selector.into();
        let raw = json!({"version":1,"default_profile":"default","profiles":{"default":{"servers":{"f":s}}}});
        assert!(Document::parse(raw.to_string().as_bytes()).is_err());
    }
    assert!(Document::parse(br#"{"version":1,"version":1,"default_profile":"default","profiles":{"default":{"servers":{}}}}"#).is_err());
    assert!(Document::parse(&vec![b' '; 256 * 1024 + 1]).is_err());
    assert!(
        Document::parse(format!("{}0{}", "[".repeat(100), "]".repeat(100)).as_bytes()).is_err()
    );
}
#[test]
fn policy_deny_and_allow_intersection_dominate_approval() {
    let policy = Policy {
        allow: Some(BTreeSet::from(["connect".into(), "tools/call:*".into()])),
        deny: BTreeSet::from(["tools/call:delete".into()]),
        require_approval: true,
    };
    assert_eq!(policy.decide("connect"), Decision::Ask);
    assert_eq!(policy.decide("tools/call:read"), Decision::Ask);
    assert_eq!(policy.decide("tools/call:delete"), Decision::Deny);
    assert_eq!(policy.decide("ping"), Decision::Deny);
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path());
    let mut server = http_server();
    server["policy"] = json!({"allow":["connect","tools/call:*"],"deny":["tools/call:delete"],"require_approval":false});
    r.replace(doc(server)).unwrap();
    let ctx = context();
    let limit = Policy {
        allow: Some(BTreeSet::from(["tools/call:read".into()])),
        ..ceiling()
    };
    let mut manager = Manager::new(&r, None, None, limit, &ctx).unwrap();
    assert!(matches!(
        manager.prepare_connect("fixture").unwrap().approve(),
        Err(Error::Denied)
    ));
    assert_eq!(
        manager
            .prepare_call("fixture", "tools/call", params("read"))
            .unwrap()
            .decision(),
        Decision::Allow
    );
    assert!(matches!(
        manager
            .prepare_call("fixture", "tools/call", params("delete"))
            .unwrap()
            .approve(),
        Err(Error::Denied)
    ));
}
#[test]
fn default_asks_and_disabled_is_not_approvable() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path());
    r.replace(doc(http_server())).unwrap();
    let ctx = context();
    let mut m = Manager::new(&r, None, None, ceiling(), &ctx).unwrap();
    assert!(matches!(
        m.prepare_connect("fixture").unwrap().permit(),
        Err(Error::ApprovalRequired)
    ));
    r.edit_server(None, "fixture", Some(false)).unwrap();
    assert!(matches!(
        m.prepare_connect("fixture").unwrap().approve(),
        Err(Error::Denied)
    ));
}
#[test]
fn stale_or_cross_manager_permits_never_connect() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path());
    r.replace(doc(http_server())).unwrap();
    let ctx = context();
    let mut first = Manager::new(&r, None, None, ceiling(), &ctx).unwrap();
    let mut other = Manager::new(&r, None, None, ceiling(), &ctx).unwrap();
    let permission = first.prepare_connect("fixture").unwrap().approve().unwrap();
    assert!(matches!(other.execute(permission, &ctx), Err(Error::Stale)));
    let permission = first.prepare_connect("fixture").unwrap().approve().unwrap();
    r.edit_server(None, "fixture", Some(false)).unwrap();
    assert!(matches!(first.execute(permission, &ctx), Err(Error::Stale)));
    assert!(first.statuses().is_empty());
}
#[test]
fn project_trust_is_bound_to_root_path_file_and_bytes() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path());
    let projectroot = root.path().join("project");
    std::fs::create_dir(&projectroot).unwrap();
    let path = projectroot.join("mcp.json");
    std::fs::write(&path, serde_json::to_vec(&doc(http_server())).unwrap()).unwrap();
    let project = Project::new(&projectroot, Path::new("mcp.json")).unwrap();
    assert!(matches!(
        r.list(None, Some(&project), &ceiling()),
        Err(Error::TrustRequired)
    ));
    r.trust(&project).unwrap();
    assert_eq!(
        r.list(None, Some(&project), &ceiling()).unwrap()[0].source,
        "project"
    );
    std::fs::write(
        &path,
        format!("{} ", std::fs::read_to_string(&path).unwrap()),
    )
    .unwrap();
    assert!(matches!(
        r.list(None, Some(&project), &ceiling()),
        Err(Error::TrustRequired)
    ));
    r.trust(&project).unwrap();
    let other = root.path().join("other");
    std::fs::create_dir(&other).unwrap();
    std::fs::copy(&path, other.join("mcp.json")).unwrap();
    let other = Project::new(&other, Path::new("mcp.json")).unwrap();
    assert!(matches!(
        r.list(None, Some(&other), &ceiling()),
        Err(Error::TrustRequired)
    ));
    r.untrust(&project).unwrap();
    assert!(matches!(
        r.list(None, Some(&project), &ceiling()),
        Err(Error::TrustRequired)
    ));
}
#[test]
fn project_cannot_relax_global_deny_allow_or_disabled() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path());
    let mut global = http_server();
    global["enabled"] = false.into();
    global["policy"] = json!({"allow":["connect","tools/call:read"],"deny":["tools/call:delete"],"require_approval":false});
    r.replace(doc(global)).unwrap();
    let mut local = http_server();
    local["transport"]["url"] = "http://127.0.0.1:2/override".into();
    local["policy"] = json!({"require_approval":false});
    std::fs::write(
        root.path().join("project.json"),
        serde_json::to_vec(&doc(local)).unwrap(),
    )
    .unwrap();
    let project = Project::new(root.path(), Path::new("project.json")).unwrap();
    r.trust(&project).unwrap();
    let ctx = context();
    let mut m = Manager::new(&r, None, Some(project), ceiling(), &ctx).unwrap();
    assert_eq!(
        m.prepare_connect("fixture").unwrap().decision(),
        Decision::Deny
    );
    r.edit_server(None, "fixture", Some(true)).unwrap();
    assert_eq!(
        m.prepare_call("fixture", "tools/call", params("read"))
            .unwrap()
            .decision(),
        Decision::Allow
    );
    assert_eq!(
        m.prepare_call("fixture", "tools/call", params("write"))
            .unwrap()
            .decision(),
        Decision::Deny
    );
    assert_eq!(
        m.prepare_call("fixture", "tools/call", params("delete"))
            .unwrap()
            .decision(),
        Decision::Deny
    );
}
#[test]
fn explicit_project_path_rejects_traversal_fifo_and_symlink_escape() {
    let root = tempfile::tempdir().unwrap();
    assert!(Project::new(root.path(), Path::new("../out.json")).is_err());
    assert!(Project::new(root.path(), Path::new("/tmp/out.json")).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let r = registry(root.path());
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(
            outside.path().join("outside.json"),
            serde_json::to_vec(&Document::default()).unwrap(),
        )
        .unwrap();
        symlink(outside.path(), root.path().join("escape")).unwrap();
        let project = Project::new(root.path(), Path::new("escape/outside.json")).unwrap();
        assert!(r.trust(&project).is_err());
        let fifo = root.path().join("fifo");
        let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let project = Project::new(root.path(), Path::new("fifo")).unwrap();
        assert!(r.trust(&project).is_err());
    }
}
#[test]
fn independent_cancellation_context_rejected_before_effect() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path());
    r.replace(doc(http_server())).unwrap();
    let ctx = context();
    let mut m = Manager::new(&r, None, None, ceiling(), &ctx).unwrap();
    let permit = m.prepare_connect("fixture").unwrap().approve().unwrap();
    assert!(matches!(
        m.execute(permit, &context()),
        Err(Error::Configuration)
    ));
    assert!(m.statuses().is_empty());
}
#[test]
fn malformed_or_background_operations_denied_before_transport() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path());
    r.replace(doc(http_server())).unwrap();
    let ctx = context();
    let mut m = Manager::new(&r, None, None, ceiling(), &ctx).unwrap();
    for method in [
        "initialize",
        "roots/list",
        "sampling/createMessage",
        "tasks/get",
        "unknown",
    ] {
        assert!(matches!(
            m.prepare_call("fixture", method, None),
            Err(Error::Denied)
        ));
    }
    assert!(m.prepare_call("fixture", "tools/call", None).is_err());
    assert!(
        m.prepare_call(
            "fixture",
            "tools/call",
            Some(
                json!({"name":"safe","task":{}})
                    .as_object()
                    .unwrap()
                    .clone()
            )
        )
        .is_err()
    );
    assert!(m.statuses().is_empty());
}
#[cfg(unix)]
fn stdio_server(root: &Path, mode: &str) -> Value {
    json!({"transport":{"type":"stdio","program":"/usr/bin/python3","args":[format!("{}/tests/fixtures/stdio_server.py",env!("CARGO_MANIFEST_DIR")),mode],"cwd":root},"policy":{"require_approval":false}})
}
#[cfg(unix)]
#[test]
fn real_stdio_connect_call_per_tool_deny_and_close() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path());
    let mut server = stdio_server(root.path(), "normal");
    server["policy"]["deny"] = json!(["tools/call:delete"]);
    r.replace(doc(server)).unwrap();
    let ctx = context();
    let mut m = Manager::new(&r, None, None, ceiling(), &ctx).unwrap();
    assert!(!root.path().join("leader.pid").exists());
    let permit = m.prepare_connect("fixture").unwrap().permit().unwrap();
    assert!(matches!(
        m.execute(permit, &ctx).unwrap(),
        Outcome::Connected
    ));
    let permit = m
        .prepare_call("fixture", "tools/call", params("read"))
        .unwrap()
        .permit()
        .unwrap();
    assert!(matches!(m.execute(permit,&ctx).unwrap(),Outcome::Reply(Ok(r)) if r["answer"]==42));
    assert!(matches!(
        m.prepare_call("fixture", "tools/call", params("delete"))
            .unwrap()
            .approve(),
        Err(Error::Denied)
    ));
    assert_eq!(
        std::fs::read_to_string(root.path().join("effects")).unwrap(),
        "1"
    );
    m.close(&ctx).unwrap();
    assert!(m.statuses().is_empty());
}
#[cfg(unix)]
#[test]
fn revocation_retires_owned_connection_before_next_effect() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path());
    r.replace(doc(stdio_server(root.path(), "normal"))).unwrap();
    let ctx = context();
    let mut m = Manager::new(&r, None, None, ceiling(), &ctx).unwrap();
    let permit = m.prepare_connect("fixture").unwrap().permit().unwrap();
    m.execute(permit, &ctx).unwrap();
    let permit = m
        .prepare_call("fixture", "ping", None)
        .unwrap()
        .permit()
        .unwrap();
    r.edit_server(None, "fixture", Some(false)).unwrap();
    assert!(matches!(m.execute(permit, &ctx), Err(Error::Stale)));
    assert!(m.statuses().is_empty());
    assert!(!root.path().join("effects").exists());
}
#[cfg(unix)]
#[test]
fn uncertain_stdio_effect_is_inspectable_and_never_reconnected() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path());
    r.replace(doc(stdio_server(root.path(), "bad_id"))).unwrap();
    let ctx = context();
    let mut m = Manager::new(&r, None, None, ceiling(), &ctx).unwrap();
    let p = m.prepare_connect("fixture").unwrap().permit().unwrap();
    m.execute(p, &ctx).unwrap();
    let p = m
        .prepare_call("fixture", "tools/call", params("read"))
        .unwrap()
        .permit()
        .unwrap();
    assert!(m.execute(p, &ctx).is_err());
    assert_eq!(m.abandoned()["fixture"][0].method, "tools/call");
    let p = m.prepare_connect("fixture").unwrap().permit().unwrap();
    assert!(matches!(m.execute(p, &ctx), Err(Error::Uncertain)));
    assert_eq!(
        std::fs::read_to_string(root.path().join("effects")).unwrap(),
        "1"
    );
}
#[cfg(unix)]
#[test]
fn cancelling_initialize_context_does_not_cancel_connection_lifetime() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path());
    r.replace(doc(stdio_server(root.path(), "normal"))).unwrap();
    let ctx = context();
    let handshake = ctx.child(Duration::from_secs(1)).unwrap();
    let mut m = Manager::new(&r, None, None, ceiling(), &ctx).unwrap();
    let permit = m.prepare_connect("fixture").unwrap().permit().unwrap();
    m.execute(permit, &handshake).unwrap();
    handshake.cancellation().cancel();
    let permit = m
        .prepare_call("fixture", "ping", None)
        .unwrap()
        .permit()
        .unwrap();
    assert!(matches!(
        m.execute(permit, &ctx).unwrap(),
        Outcome::Reply(Ok(_))
    ));
    m.close(&ctx).unwrap();
}
#[cfg(unix)]
#[test]
fn caller_child_cancellation_interrupts_real_call_and_preserves_parent() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path());
    r.replace(doc(stdio_server(root.path(), "wait"))).unwrap();
    let ctx = context();
    let call = ctx.child(Duration::from_secs(5)).unwrap();
    let mut m = Manager::new(&r, None, None, ceiling(), &ctx).unwrap();
    let p = m.prepare_connect("fixture").unwrap().permit().unwrap();
    m.execute(p, &ctx).unwrap();
    let signal = call.cancellation();
    let observed = root.path().join("effects");
    let worker = std::thread::spawn(move || {
        let until = std::time::Instant::now() + Duration::from_secs(4);
        while !observed.exists() {
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        }
        signal.cancel();
    });
    let p = m
        .prepare_call("fixture", "ping", None)
        .unwrap()
        .permit()
        .unwrap();
    assert!(matches!(
        m.execute(p, &call),
        Err(Error::Stdio(namzu_mcp::stdio::Error::Context(
            namzu_core::AgentError::Cancelled
        )))
    ));
    worker.join().unwrap();
    assert!(ctx.remaining().is_ok());
    assert_eq!(m.statuses()[0].phase, "closed");
    assert_eq!(m.abandoned()["fixture"].len(), 1);
}
#[test]
fn project_untrust_invalidates_prepared_permission() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path());
    std::fs::write(
        root.path().join("project.json"),
        serde_json::to_vec(&doc(http_server())).unwrap(),
    )
    .unwrap();
    let project = Project::new(root.path(), Path::new("project.json")).unwrap();
    r.trust(&project).unwrap();
    let ctx = context();
    let mut m = Manager::new(&r, None, Some(project.clone()), ceiling(), &ctx).unwrap();
    let permit = m.prepare_connect("fixture").unwrap().approve().unwrap();
    r.untrust(&project).unwrap();
    assert!(matches!(m.execute(permit, &ctx), Err(Error::TrustRequired)));
    assert!(m.statuses().is_empty());
}
#[test]
fn http_manager_locks_admission_and_enforces_per_operation_approval() {
    http_manager_fixture(0);
}
#[test]
fn failed_http_session_retains_uncertainty_and_can_be_explicitly_deleted() {
    http_manager_fixture(1);
}
#[test]
fn config_revocation_deletes_old_http_session_before_new_admission() {
    http_manager_fixture(2);
}
fn http_manager_fixture(mode: u8) {
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path());
    let mut server = http_server();
    server["transport"]["url"] = format!("http://{address}/mcp").into();
    r.replace(doc(server)).unwrap();
    let state = root.path().join("state");
    let worker = std::thread::spawn(move || {
        for index in 0..4 {
            let until = std::time::Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(std::time::Instant::now() < until);
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut headers = vec![];
            let mut b = [0];
            while !headers.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut b).unwrap();
                headers.push(b[0]);
                assert!(headers.len() < 16384);
            }
            let headers = String::from_utf8(headers).unwrap();
            let len = headers
                .lines()
                .find_map(|line| {
                    line.to_lowercase()
                        .strip_prefix("content-length: ")
                        .map(|s| s.parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            let mut body = vec![0; len];
            stream.read_exact(&mut body).unwrap();
            let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let registry = Registry::open(&state).unwrap();
            if index < 3 || mode == 2 {
                assert!(
                    registry.edit_server(None, "fixture", Some(false)).is_err(),
                    "admission lock must cover effects"
                );
            } else {
                assert!(
                    registry.edit_server(None, "fixture", Some(false)).is_ok(),
                    "cleanup must remain independent of permission changes"
                );
            }
            let (status, body) = match index {
                0 => {
                    assert_eq!(request["method"], "initialize");
                    (200,json!({"jsonrpc":"2.0","id":request["id"],"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}}}).to_string())
                }
                1 => {
                    assert_eq!(request["method"], "notifications/initialized");
                    (202, String::new())
                }
                2 => {
                    assert_eq!(request["params"]["name"], "read");
                    if mode == 1 {
                        (404, String::new())
                    } else {
                        (
                            200,
                            json!({"jsonrpc":"2.0","id":request["id"],"result":{"answer":42}})
                                .to_string(),
                        )
                    }
                }
                _ => {
                    assert!(headers.starts_with("DELETE "));
                    (204, String::new())
                }
            };
            write!(stream,"HTTP/1.1 {status} fixture\r\nContent-Type: application/json\r\nMCP-Session-Id: manager-fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        }
    });
    let ctx = context();
    let mut m = Manager::new(&r, None, None, ceiling(), &ctx).unwrap();
    assert!(matches!(
        m.prepare_connect("fixture").unwrap().permit(),
        Err(Error::ApprovalRequired)
    ));
    let p = m.prepare_connect("fixture").unwrap().approve().unwrap();
    m.execute(p, &ctx).unwrap();
    assert!(matches!(
        m.prepare_call("fixture", "tools/call", params("read"))
            .unwrap()
            .permit(),
        Err(Error::ApprovalRequired)
    ));
    let p = m
        .prepare_call("fixture", "tools/call", params("read"))
        .unwrap()
        .approve()
        .unwrap();
    let result = m.execute(p, &ctx);
    if mode == 1 {
        assert!(matches!(
            result,
            Err(Error::Http(namzu_mcp::http::Error::SessionExpired))
        ));
        assert_eq!(m.abandoned()["fixture"][0].method, "tools/call");
        assert_eq!(m.statuses()[0].phase, "closed");
    } else {
        assert!(matches!(result.unwrap(),Outcome::Reply(Ok(result)) if result["answer"]==42));
    }
    if mode == 2 {
        r.edit_server(None, "fixture", Some(false)).unwrap();
        assert_eq!(
            m.prepare_connect("fixture").unwrap().decision(),
            Decision::Deny
        );
        assert!(m.statuses().is_empty());
    } else {
        m.close(&ctx).unwrap();
    }
    worker.join().unwrap();
}
