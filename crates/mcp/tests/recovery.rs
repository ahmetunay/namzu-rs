#![cfg(unix)]
use namzu_core::{Cancellation, TurnContext};
use namzu_mcp::{
    config::{Document, Error, Policy, Registry},
    discovery::Kind,
    manager::{Manager, Outcome},
};
use serde_json::{Value, json};
use std::{
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
fn ctx() -> TurnContext {
    TurnContext::new(Duration::from_secs(300), Cancellation::default()).unwrap()
}
fn ceiling() -> Policy {
    Policy {
        require_approval: false,
        ..Policy::default()
    }
}
fn registry(root: &Path) -> Registry {
    let r = Registry::open(&root.join("state")).unwrap();
    let server = json!({"transport":{"type":"stdio","program":"/usr/bin/python3","args":[format!("{}/tests/fixtures/recovery_server.py",env!("CARGO_MANIFEST_DIR"))],"cwd":root},"policy":{"require_approval":false}});
    r.replace(Document::parse(json!({"version":1,"default_profile":"default","profiles":{"default":{"servers":{"fixture":server}}}}).to_string().as_bytes()).unwrap()).unwrap();
    r
}
fn connect(m: &mut Manager<'_>, ctx: &TurnContext) {
    let p = m.prepare_connect("fixture").unwrap().permit().unwrap();
    m.execute(p, ctx).unwrap();
}
fn call(
    m: &mut Manager<'_>,
    method: &str,
    params: Value,
    ctx: &TurnContext,
) -> Result<Outcome, Error> {
    let p = m
        .prepare_call("fixture", method, params.as_object().cloned())
        .unwrap()
        .permit()
        .unwrap();
    m.execute(p, ctx)
}
fn mode(root: &Path, value: &str) {
    std::fs::write(root.join("mode"), value).unwrap();
}
fn count(root: &Path, method: &str) -> usize {
    std::fs::read_to_string(root.join("requests.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter(|line| serde_json::from_str::<Value>(line).unwrap()["method"] == method)
        .count()
}
fn effects(root: &Path) -> usize {
    std::fs::read_to_string(root.join("effects"))
        .unwrap_or_default()
        .lines()
        .count()
}
#[test]
fn explicit_reconnect_retires_all_old_permissions_catalogs_and_subscriptions() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path());
    let c = ctx();
    let mut m = Manager::new(&r, None, None, ceiling(), &c).unwrap();
    connect(&mut m, &c);
    m.discover("fixture", Kind::Tools, &c, |p| p.permit())
        .unwrap();
    call(
        &mut m,
        "resources/subscribe",
        json!({"uri":"mem://fixture/data"}),
        &c,
    )
    .unwrap();
    let stale = m
        .prepare_call("fixture", "ping", None)
        .unwrap()
        .permit()
        .unwrap();
    let before = m.statuses()[0].generation;
    let p = m.prepare_reconnect("fixture").unwrap().permit().unwrap();
    m.execute(p, &c).unwrap();
    assert!(m.statuses()[0].generation > before);
    assert!(m.catalog("fixture", Kind::Tools).unwrap().is_none());
    assert!(m.subscriptions("fixture").unwrap().is_empty());
    assert!(matches!(m.execute(stale, &c), Err(Error::Stale)));
    call(&mut m, "ping", Value::Null, &c).unwrap();
    m.close(&c).unwrap();
    assert_eq!(count(root.path(), "initialize"), 2);
    assert_eq!(count(root.path(), "resources/subscribe"), 1);
    assert_eq!(count(root.path(), "tools/call"), 0);
}
#[test]
fn readonly_disconnect_can_reconnect_explicitly_without_replaying_ping() {
    let root = tempfile::tempdir().unwrap();
    mode(root.path(), "drop_ping");
    let r = registry(root.path());
    let c = ctx();
    let mut m = Manager::new(&r, None, None, ceiling(), &c).unwrap();
    connect(&mut m, &c);
    assert!(call(&mut m, "ping", Value::Null, &c).is_err());
    assert!(r.recovery().unwrap().list().unwrap().is_empty());
    assert_eq!(count(root.path(), "ping"), 1);
    let p = m.prepare_reconnect("fixture").unwrap().permit().unwrap();
    m.execute(p, &c).unwrap();
    assert_eq!(count(root.path(), "ping"), 1);
    call(&mut m, "ping", Value::Null, &c).unwrap();
    assert_eq!(count(root.path(), "ping"), 2);
    m.close(&c).unwrap();
}
#[test]
fn tool_disconnect_fences_new_managers_until_explicit_inspection() {
    let root = tempfile::tempdir().unwrap();
    mode(root.path(), "drop_tool");
    let r = registry(root.path());
    let c = ctx();
    let mut m = Manager::new(&r, None, None, ceiling(), &c).unwrap();
    connect(&mut m, &c);
    assert!(
        call(
            &mut m,
            "tools/call",
            json!({"name":"effect","arguments":{"secret":"PRIVATE_FIXTURE_ARGUMENT"}}),
            &c
        )
        .is_err()
    );
    let records = r.recovery().unwrap().list().unwrap();
    assert_eq!(records.len(), 1);
    assert!(records[0].uncertain && !records[0].active);
    let encoded = serde_json::to_string(&records).unwrap();
    assert!(!encoded.contains("PRIVATE_FIXTURE_ARGUMENT"));
    let p = m.prepare_reconnect("fixture").unwrap().permit().unwrap();
    assert!(matches!(m.execute(p, &c), Err(Error::Uncertain)));
    m.close(&c).unwrap();
    drop(m);
    let mut m = Manager::new(&r, None, None, ceiling(), &c).unwrap();
    let p = m.prepare_connect("fixture").unwrap().permit().unwrap();
    assert!(matches!(m.execute(p, &c), Err(Error::Uncertain)));
    assert_eq!(effects(root.path()), 1);
    r.recovery().unwrap().acknowledge(&records[0].id).unwrap();
    mode(root.path(), "normal");
    connect(&mut m, &c);
    assert_eq!(effects(root.path()), 1);
    call(
        &mut m,
        "tools/call",
        json!({"name":"effect","arguments":{}}),
        &c,
    )
    .unwrap();
    assert_eq!(effects(root.path()), 2);
    assert!(r.recovery().unwrap().list().unwrap().is_empty());
    m.close(&c).unwrap();
}
#[test]
fn successful_effect_removes_its_pending_record() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path());
    let c = ctx();
    let mut m = Manager::new(&r, None, None, ceiling(), &c).unwrap();
    connect(&mut m, &c);
    call(
        &mut m,
        "tools/call",
        json!({"name":"effect","arguments":{}}),
        &c,
    )
    .unwrap();
    assert!(r.recovery().unwrap().list().unwrap().is_empty());
    m.close(&c).unwrap();
}
#[test]
fn resource_updates_are_coalesced_only_for_active_generation_and_uri() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path());
    let c = ctx();
    let mut m = Manager::new(&r, None, None, ceiling(), &c).unwrap();
    connect(&mut m, &c);
    let uri = "mem://fixture/data";
    call(&mut m, "resources/subscribe", json!({"uri":uri}), &c).unwrap();
    mode(root.path(), "updates");
    call(&mut m, "ping", Value::Null, &c).unwrap();
    assert_eq!(m.take_resource_updates("fixture").unwrap(), vec![uri]);
    assert!(m.take_resource_updates("fixture").unwrap().is_empty());
    let generation = m.statuses()[0].generation;
    while let Some(n) = m.pop_notification() {
        assert_eq!(n.generation, generation);
    }
    assert_eq!(count(root.path(), "resources/read"), 0);
    call(&mut m, "resources/unsubscribe", json!({"uri":uri}), &c).unwrap();
    call(&mut m, "ping", Value::Null, &c).unwrap();
    assert!(m.take_resource_updates("fixture").unwrap().is_empty());
    assert!(m.subscriptions("fixture").unwrap().is_empty());
    m.close(&c).unwrap();
}
#[test]
fn duplicate_unknown_unsubscribe_and_missing_capability_have_no_remote_effect() {
    for value in ["normal", "no_subscribe"] {
        let root = tempfile::tempdir().unwrap();
        mode(root.path(), value);
        let r = registry(root.path());
        let c = ctx();
        let mut m = Manager::new(&r, None, None, ceiling(), &c).unwrap();
        connect(&mut m, &c);
        assert!(
            call(
                &mut m,
                "resources/unsubscribe",
                json!({"uri":"mem://none"}),
                &c
            )
            .is_err()
        );
        if value == "normal" {
            call(
                &mut m,
                "resources/subscribe",
                json!({"uri":"mem://one"}),
                &c,
            )
            .unwrap();
            assert!(
                call(
                    &mut m,
                    "resources/subscribe",
                    json!({"uri":"mem://one"}),
                    &c
                )
                .is_err()
            );
            assert_eq!(count(root.path(), "resources/subscribe"), 1);
        } else {
            assert!(
                call(
                    &mut m,
                    "resources/subscribe",
                    json!({"uri":"mem://one"}),
                    &c
                )
                .is_err()
            );
            assert_eq!(count(root.path(), "resources/subscribe"), 0);
        }
        assert_eq!(count(root.path(), "resources/unsubscribe"), 0);
        m.close(&c).unwrap();
    }
}
#[test]
fn subscription_rpc_error_does_not_commit_or_remove_membership() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path());
    let c = ctx();
    let mut m = Manager::new(&r, None, None, ceiling(), &c).unwrap();
    connect(&mut m, &c);
    let p = json!({"uri":"mem://one"});
    mode(root.path(), "rpc_error");
    assert!(matches!(
        call(&mut m, "resources/subscribe", p.clone(), &c),
        Ok(Outcome::Reply(Err(_)))
    ));
    assert!(m.subscriptions("fixture").unwrap().is_empty());
    mode(root.path(), "normal");
    call(&mut m, "resources/subscribe", p.clone(), &c).unwrap();
    mode(root.path(), "rpc_error");
    assert!(matches!(
        call(&mut m, "resources/unsubscribe", p, &c),
        Ok(Outcome::Reply(Err(_)))
    ));
    assert_eq!(m.subscriptions("fixture").unwrap(), vec!["mem://one"]);
    m.close(&c).unwrap();
}
#[test]
fn malformed_update_fences_subscription_and_pending_old_permissions() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path());
    let c = ctx();
    let mut m = Manager::new(&r, None, None, ceiling(), &c).unwrap();
    connect(&mut m, &c);
    call(
        &mut m,
        "resources/subscribe",
        json!({"uri":"mem://one"}),
        &c,
    )
    .unwrap();
    let stale = m
        .prepare_call("fixture", "ping", None)
        .unwrap()
        .permit()
        .unwrap();
    mode(root.path(), "malformed_update");
    assert!(call(&mut m, "ping", Value::Null, &c).is_err());
    assert!(m.subscriptions("fixture").unwrap().is_empty());
    assert!(matches!(m.execute(stale, &c), Err(Error::Stale)));
    m.close(&c).unwrap();
}
#[test]
fn close_invalidates_even_a_never_dispatched_connect_permit() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path());
    let c = ctx();
    let mut m = Manager::new(&r, None, None, ceiling(), &c).unwrap();
    let p = m.prepare_connect("fixture").unwrap().permit().unwrap();
    m.close(&c).unwrap();
    assert!(matches!(m.execute(p, &c), Err(Error::Stale)));
    assert_eq!(count(root.path(), "initialize"), 0);
}
#[test]
fn reconnect_requires_connect_and_its_own_policy_admission() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path());
    let c = ctx();
    for deny in ["connect", "connection/reconnect"] {
        let mut policy = ceiling();
        policy.deny.insert(deny.into());
        let mut m = Manager::new(&r, None, None, policy, &c).unwrap();
        assert!(matches!(
            m.prepare_reconnect("fixture").unwrap().approve(),
            Err(Error::Denied)
        ));
    }
    assert_eq!(count(root.path(), "initialize"), 0);
}
#[test]
fn negotiated_older_peer_can_reconnect_and_use_resources() {
    let root = tempfile::tempdir().unwrap();
    mode(root.path(), "legacy");
    let r = registry(root.path());
    let c = ctx();
    let mut m = Manager::new(&r, None, None, ceiling(), &c).unwrap();
    connect(&mut m, &c);
    call(
        &mut m,
        "resources/subscribe",
        json!({"uri":"mem://one"}),
        &c,
    )
    .unwrap();
    let p = m.prepare_reconnect("fixture").unwrap().permit().unwrap();
    m.execute(p, &c).unwrap();
    call(&mut m, "resources/read", json!({"uri":"mem://one"}), &c).unwrap();
    m.close(&c).unwrap();
    assert_eq!(count(root.path(), "initialize"), 2);
}
#[test]
fn recovery_host_child() {
    let Ok(root) = std::env::var("NAMZU_MCP_RECOVERY_FIXTURE_ROOT") else {
        return;
    };
    let r = Registry::open(&Path::new(&root).join("state")).unwrap();
    let c = ctx();
    let mut m = Manager::new(&r, None, None, ceiling(), &c).unwrap();
    connect(&mut m, &c);
    let _ = call(
        &mut m,
        "tools/call",
        json!({"name":"effect","arguments":{"secret":"PRIVATE_FIXTURE_ARGUMENT"}}),
        &c,
    );
    panic!("parent should kill blocked fixture host");
}
#[test]
fn killed_host_pending_effect_is_durable_and_active_ack_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    mode(root.path(), "block_tool");
    let r = registry(root.path());
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "recovery_host_child", "--nocapture"])
        .env("NAMZU_MCP_RECOVERY_FIXTURE_ROOT", root.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !root.path().join("started").exists() {
        assert!(Instant::now() < deadline);
        assert!(child.try_wait().unwrap().is_none());
        std::thread::sleep(Duration::from_millis(2));
    }
    let store = r.recovery().unwrap();
    let records = store.list().unwrap();
    assert_eq!(records.len(), 1);
    assert!(records[0].active && !records[0].uncertain);
    assert!(matches!(
        store.acknowledge(&records[0].id),
        Err(Error::RecoveryActive)
    ));
    child.kill().unwrap();
    child.wait().unwrap();
    let records = store.list().unwrap();
    assert!(records[0].uncertain && !records[0].active);
    let c = ctx();
    let mut m = Manager::new(&r, None, None, ceiling(), &c).unwrap();
    let p = m.prepare_connect("fixture").unwrap().permit().unwrap();
    assert!(matches!(m.execute(p, &c), Err(Error::Uncertain)));
    assert_eq!(effects(root.path()), 1);
    store.acknowledge(&records[0].id).unwrap();
    mode(root.path(), "normal");
    connect(&mut m, &c);
    assert_eq!(effects(root.path()), 1);
    m.close(&c).unwrap();
}
#[test]
fn subscription_capacity_is_enforced_before_the_129th_remote_request() {
    let root = tempfile::tempdir().unwrap();
    mode(root.path(), "stall_events");
    let r = registry(root.path());
    let c = ctx();
    let mut m = Manager::new(&r, None, None, ceiling(), &c).unwrap();
    connect(&mut m, &c);
    for i in 0..128 {
        call(
            &mut m,
            "resources/subscribe",
            json!({"uri":format!("mem://{i}")}),
            &c,
        )
        .unwrap();
    }
    assert!(matches!(
        call(
            &mut m,
            "resources/subscribe",
            json!({"uri":"mem://overflow"}),
            &c
        ),
        Err(Error::Subscription)
    ));
    assert_eq!(count(root.path(), "resources/subscribe"), 128);
    assert_eq!(m.subscriptions("fixture").unwrap().len(), 128);
    m.close(&c).unwrap();
}
#[test]
fn standard_request_metadata_is_preserved_for_resource_subscriptions() {
    let root = tempfile::tempdir().unwrap();
    mode(root.path(), "stall_events");
    let r = registry(root.path());
    let c = ctx();
    let mut m = Manager::new(&r, None, None, ceiling(), &c).unwrap();
    connect(&mut m, &c);
    call(
        &mut m,
        "resources/subscribe",
        json!({"uri":"mem://one","_meta":{"progressToken":"fixture-progress"}}),
        &c,
    )
    .unwrap();
    assert_eq!(m.subscriptions("fixture").unwrap(), vec!["mem://one"]);
    assert!(
        call(
            &mut m,
            "resources/subscribe",
            json!({"uri":"mem://two","_meta":42}),
            &c
        )
        .is_err()
    );
    assert_eq!(count(root.path(), "resources/subscribe"), 1);
    m.close(&c).unwrap();
}
#[test]
fn correlated_rpc_failure_is_known_and_does_not_leave_a_durable_fence() {
    let root = tempfile::tempdir().unwrap();
    mode(root.path(), "rpc_tool_error");
    let r = registry(root.path());
    let c = ctx();
    let mut m = Manager::new(&r, None, None, ceiling(), &c).unwrap();
    connect(&mut m, &c);
    assert!(matches!(
        call(
            &mut m,
            "tools/call",
            json!({"name":"effect","arguments":{}}),
            &c
        ),
        Ok(Outcome::Reply(Err(_)))
    ));
    assert!(r.recovery().unwrap().list().unwrap().is_empty());
    assert_eq!(effects(root.path()), 0);
    mode(root.path(), "normal");
    call(
        &mut m,
        "tools/call",
        json!({"name":"effect","arguments":{}}),
        &c,
    )
    .unwrap();
    assert_eq!(effects(root.path()), 1);
    m.close(&c).unwrap();
}
