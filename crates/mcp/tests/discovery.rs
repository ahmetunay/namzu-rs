#![cfg(unix)]
use namzu_core::{AgentError, Cancellation, TurnContext};
use namzu_mcp::{
    config::{Document, Error, Policy, Registry},
    discovery::{Error as DiscoveryError, Kind, prompt_result, read_result, tool_result},
    manager::{Manager, Outcome},
};
use serde_json::{Map, Value, json};
use std::{
    path::Path,
    time::{Duration, Instant},
};
fn context() -> TurnContext {
    TurnContext::new(Duration::from_secs(10), Cancellation::default()).unwrap()
}
fn ceiling() -> Policy {
    Policy {
        require_approval: false,
        ..Policy::default()
    }
}
fn server(root: &Path, mode: &str) -> Value {
    json!({"transport":{"type":"stdio","program":"/usr/bin/python3","args":[format!("{}/tests/fixtures/discovery_server.py",env!("CARGO_MANIFEST_DIR")),mode],"cwd":root},"policy":{"require_approval":false}})
}
fn registry(root: &Path, mode: &str) -> Registry {
    let registry = Registry::open(&root.join("state")).unwrap();
    let doc = json!({"version":1,"default_profile":"default","profiles":{"default":{"servers":{"fixture":server(root,mode)}}}});
    registry
        .replace(Document::parse(doc.to_string().as_bytes()).unwrap())
        .unwrap();
    registry
}
fn connect(m: &mut Manager<'_>, server: &str, ctx: &TurnContext) {
    let permit = m.prepare_connect(server).unwrap().permit().unwrap();
    m.execute(permit, ctx).unwrap();
}
fn requests(root: &Path) -> Vec<Value> {
    std::fs::read_to_string(root.join("requests.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
fn count(root: &Path, method: &str) -> usize {
    requests(root)
        .iter()
        .filter(|r| r["method"] == method)
        .count()
}
#[test]
fn real_stdio_discovers_all_kinds_and_typed_operations() {
    let root = tempfile::tempdir().unwrap();
    let registry = registry(root.path(), "normal");
    let ctx = context();
    let mut manager = Manager::new(&registry, None, None, ceiling(), &ctx).unwrap();
    connect(&mut manager, "fixture", &ctx);
    for kind in [Kind::Tools, Kind::Resources, Kind::Prompts, Kind::Templates] {
        let catalog = manager
            .discover("fixture", kind, &ctx, |plan| plan.permit())
            .unwrap();
        assert_eq!(catalog.server, "fixture");
        assert_eq!(catalog.items.len(), if kind == Kind::Tools { 2 } else { 1 });
    }
    let permit = manager
        .prepare_tool_call(
            "fixture",
            "read",
            json!({"path":"a"}).as_object().unwrap().clone(),
        )
        .unwrap()
        .permit()
        .unwrap();
    let Outcome::Reply(Ok(result)) = manager.execute(permit, &ctx).unwrap() else {
        panic!()
    };
    assert_eq!(
        tool_result(result).unwrap().structured_content.unwrap()["answer"],
        42
    );
    let permit = manager
        .prepare_resource_read("fixture", "mem://resource/α")
        .unwrap()
        .permit()
        .unwrap();
    let Outcome::Reply(Ok(result)) = manager.execute(permit, &ctx).unwrap() else {
        panic!()
    };
    assert_eq!(
        read_result(result).unwrap().contents[0].text.as_deref(),
        Some("hello")
    );
    assert!(
        manager
            .prepare_prompt_get("fixture", "review", Map::new())
            .is_err()
    );
    let permit = manager
        .prepare_prompt_get(
            "fixture",
            "review",
            json!({"path":"a"}).as_object().unwrap().clone(),
        )
        .unwrap()
        .permit()
        .unwrap();
    let Outcome::Reply(Ok(result)) = manager.execute(permit, &ctx).unwrap() else {
        panic!()
    };
    assert_eq!(prompt_result(result).unwrap().messages[0].role, "user");
    assert!(
        manager
            .prepare_tool_call("fixture", "not-advertised", Map::new())
            .is_err()
    );
    manager.close(&ctx).unwrap();
    assert_eq!(count(root.path(), "tools/list"), 2);
}
fn malformed(mode: &str, expected: DiscoveryError) {
    let root = tempfile::tempdir().unwrap();
    let registry = registry(root.path(), mode);
    let ctx = context();
    let mut m = Manager::new(&registry, None, None, ceiling(), &ctx).unwrap();
    connect(&mut m, "fixture", &ctx);
    assert!(
        matches!(m.discover("fixture",Kind::Tools,&ctx,|p|p.permit()),Err(Error::Discovery(e)) if e==expected)
    );
    assert!(m.catalog("fixture", Kind::Tools).unwrap().is_none());
    m.close(&ctx).unwrap();
    assert_eq!(count(root.path(), "tools/call"), 0);
}
#[test]
fn duplicate_cursor_rejects_whole_catalog() {
    malformed("duplicate_cursor", DiscoveryError::Cursor);
}
#[test]
fn duplicate_item_rejects_whole_catalog() {
    malformed("duplicate_item", DiscoveryError::Duplicate);
}
#[test]
fn malformed_page_rejects_whole_catalog() {
    malformed("malformed", DiscoveryError::Payload);
}
#[test]
fn invalid_schema_rejects_whole_catalog() {
    malformed("bad_schema", DiscoveryError::Schema);
}
#[test]
fn remote_schema_ref_rejected_without_fetching() {
    malformed("remote_ref", DiscoveryError::Schema);
}
#[test]
fn invalid_tool_name_rejects_whole_catalog() {
    malformed("bad_name", DiscoveryError::Payload);
}
#[test]
fn excessive_items_rejected() {
    malformed("huge", DiscoveryError::Limit);
}
#[test]
fn rpc_error_redacted_and_never_published() {
    malformed("rpc_error", DiscoveryError::Rpc);
}
#[test]
fn list_change_during_pagination_aborts_without_retry() {
    malformed("change_during", DiscoveryError::Stale);
}
#[test]
fn page_budget_is_checked_before_a_33rd_rpc() {
    let root = tempfile::tempdir().unwrap();
    let registry = registry(root.path(), "too_many_pages");
    let ctx = context();
    let mut m = Manager::new(&registry, None, None, ceiling(), &ctx).unwrap();
    connect(&mut m, "fixture", &ctx);
    assert!(matches!(
        m.discover("fixture", Kind::Tools, &ctx, |p| p.permit()),
        Err(Error::Discovery(DiscoveryError::Limit))
    ));
    m.close(&ctx).unwrap();
    assert_eq!(count(root.path(), "tools/list"), 32);
}
#[test]
fn discovery_deny_or_missing_capability_sends_no_list_rpc() {
    for absent in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let registry = registry(root.path(), if absent { "no_tools" } else { "normal" });
        let ctx = context();
        let limit = Policy {
            deny: if absent {
                Default::default()
            } else {
                ["tools/list".into()].into_iter().collect()
            },
            ..ceiling()
        };
        let mut m = Manager::new(&registry, None, None, limit, &ctx).unwrap();
        connect(&mut m, "fixture", &ctx);
        assert!(
            m.discover("fixture", Kind::Tools, &ctx, |p| p.approve())
                .is_err()
        );
        m.close(&ctx).unwrap();
        assert_eq!(count(root.path(), "tools/list"), 0);
    }
}
#[test]
fn per_page_approval_cannot_be_reused_or_replace_intent() {
    let root = tempfile::tempdir().unwrap();
    let registry = registry(root.path(), "normal");
    let ctx = context();
    let mut m = Manager::new(&registry, None, None, ceiling(), &ctx).unwrap();
    connect(&mut m, "fixture", &ctx);
    let mut pages = 0;
    assert!(matches!(
        m.discover("fixture", Kind::Tools, &ctx, |p| {
            pages += 1;
            if pages == 2 {
                Err(Error::ApprovalRequired)
            } else {
                p.approve()
            }
        }),
        Err(Error::ApprovalRequired)
    ));
    assert!(m.catalog("fixture", Kind::Tools).unwrap().is_none());
    assert_eq!(count(root.path(), "tools/list"), 1);
    let mut unrelated = Some(
        m.prepare_call("fixture", "ping", None)
            .unwrap()
            .permit()
            .unwrap(),
    );
    assert!(matches!(
        m.discover("fixture", Kind::Tools, &ctx, |_| Ok(unrelated
            .take()
            .unwrap())),
        Err(Error::Stale)
    ));
    m.close(&ctx).unwrap();
    assert_eq!(count(root.path(), "ping"), 0);
}
#[test]
fn announced_change_invalidates_catalog_and_prepared_tool_grant() {
    let root = tempfile::tempdir().unwrap();
    let registry = registry(root.path(), "notify");
    let ctx = context();
    let mut m = Manager::new(&registry, None, None, ceiling(), &ctx).unwrap();
    connect(&mut m, "fixture", &ctx);
    m.discover("fixture", Kind::Tools, &ctx, |p| p.permit())
        .unwrap();
    let stale = m
        .prepare_tool_call("fixture", "read", Map::new())
        .unwrap()
        .permit()
        .unwrap();
    let ping = m
        .prepare_call("fixture", "ping", None)
        .unwrap()
        .permit()
        .unwrap();
    m.execute(ping, &ctx).unwrap();
    assert!(m.catalog("fixture", Kind::Tools).unwrap().is_none());
    assert!(matches!(
        m.execute(stale, &ctx),
        Err(Error::Discovery(DiscoveryError::Stale))
    ));
    assert!(m.prepare_tool_call("fixture", "read", Map::new()).is_err());
    assert_eq!(
        m.pop_notification().unwrap().method,
        "notifications/tools/list_changed"
    );
    m.close(&ctx).unwrap();
    assert_eq!(count(root.path(), "tools/call"), 0);
}
#[test]
fn unrelated_or_unannounced_changes_do_not_invalidate_tools() {
    for mode in ["unrelated_change", "unannounced"] {
        let root = tempfile::tempdir().unwrap();
        let registry = registry(root.path(), mode);
        let ctx = context();
        let mut m = Manager::new(&registry, None, None, ceiling(), &ctx).unwrap();
        connect(&mut m, "fixture", &ctx);
        m.discover("fixture", Kind::Tools, &ctx, |p| p.permit())
            .unwrap();
        let p = m
            .prepare_call("fixture", "ping", None)
            .unwrap()
            .permit()
            .unwrap();
        m.execute(p, &ctx).unwrap();
        assert!(m.catalog("fixture", Kind::Tools).unwrap().is_some());
        m.close(&ctx).unwrap();
    }
}
#[test]
fn resources_change_invalidates_resources_and_templates_only() {
    let root = tempfile::tempdir().unwrap();
    let registry = registry(root.path(), "resource_notify");
    let ctx = context();
    let mut m = Manager::new(&registry, None, None, ceiling(), &ctx).unwrap();
    connect(&mut m, "fixture", &ctx);
    for kind in [Kind::Resources, Kind::Templates, Kind::Tools] {
        m.discover("fixture", kind, &ctx, |p| p.permit()).unwrap();
    }
    let p = m
        .prepare_call("fixture", "ping", None)
        .unwrap()
        .permit()
        .unwrap();
    m.execute(p, &ctx).unwrap();
    assert!(m.catalog("fixture", Kind::Resources).unwrap().is_none());
    assert!(m.catalog("fixture", Kind::Templates).unwrap().is_none());
    assert!(m.catalog("fixture", Kind::Tools).unwrap().is_some());
    m.close(&ctx).unwrap();
}
#[test]
fn explicit_stdio_notification_pump_fences_stale_grant() {
    let root = tempfile::tempdir().unwrap();
    let registry = registry(root.path(), "idle_notify");
    let ctx = context();
    let mut m = Manager::new(&registry, None, None, ceiling(), &ctx).unwrap();
    connect(&mut m, "fixture", &ctx);
    m.discover("fixture", Kind::Tools, &ctx, |p| p.permit())
        .unwrap();
    let stale = m
        .prepare_tool_call("fixture", "read", Map::new())
        .unwrap()
        .permit()
        .unwrap();
    let p = m
        .prepare_call("fixture", "ping", None)
        .unwrap()
        .permit()
        .unwrap();
    m.execute(p, &ctx).unwrap();
    let p = m
        .prepare_notifications("fixture")
        .unwrap()
        .permit()
        .unwrap();
    assert!(matches!(
        m.execute(p, &ctx).unwrap(),
        Outcome::Events { supported: true }
    ));
    assert!(matches!(
        m.execute(stale, &ctx),
        Err(Error::Discovery(DiscoveryError::Stale))
    ));
    m.close(&ctx).unwrap();
}
#[test]
fn update_notification_never_reads_a_resource_automatically() {
    let root = tempfile::tempdir().unwrap();
    let registry = registry(root.path(), "updated");
    let ctx = context();
    let mut m = Manager::new(&registry, None, None, ceiling(), &ctx).unwrap();
    connect(&mut m, "fixture", &ctx);
    let p = m
        .prepare_call("fixture", "ping", None)
        .unwrap()
        .permit()
        .unwrap();
    m.execute(p, &ctx).unwrap();
    assert_eq!(
        m.pop_notification().unwrap().params.unwrap()["uri"],
        "file:///DO_NOT_OPEN"
    );
    m.close(&ctx).unwrap();
    assert_eq!(count(root.path(), "resources/read"), 0);
}
#[test]
fn notification_flood_is_bounded_and_closes_generation() {
    let root = tempfile::tempdir().unwrap();
    let registry = registry(root.path(), "flood");
    let ctx = context();
    let mut m = Manager::new(&registry, None, None, ceiling(), &ctx).unwrap();
    connect(&mut m, "fixture", &ctx);
    m.discover("fixture", Kind::Tools, &ctx, |p| p.permit())
        .unwrap();
    let p = m
        .prepare_call("fixture", "ping", None)
        .unwrap()
        .permit()
        .unwrap();
    assert!(m.execute(p, &ctx).is_err());
    assert!(m.catalog("fixture", Kind::Tools).unwrap().is_none());
    assert_eq!(m.statuses()[0].phase, "closed");
    m.close(&ctx).unwrap();
}
#[test]
fn cancellation_during_second_page_does_not_publish_first_page() {
    let root = tempfile::tempdir().unwrap();
    let registry = registry(root.path(), "wait_page");
    let ctx = context();
    let call = ctx.child(Duration::from_secs(5)).unwrap();
    let mut m = Manager::new(&registry, None, None, ceiling(), &ctx).unwrap();
    connect(&mut m, "fixture", &ctx);
    let path = root.path().join("page_started");
    let signal = call.cancellation();
    let worker = std::thread::spawn(move || {
        let until = Instant::now() + Duration::from_secs(4);
        while !path.exists() {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        }
        signal.cancel();
    });
    assert!(matches!(
        m.discover("fixture", Kind::Tools, &call, |p| p.permit()),
        Err(Error::Stdio(namzu_mcp::stdio::Error::Context(
            AgentError::Cancelled
        )))
    ));
    worker.join().unwrap();
    assert!(m.catalog("fixture", Kind::Tools).unwrap().is_none());
    assert!(ctx.remaining().is_ok());
    m.close(&ctx).unwrap();
}
#[test]
fn same_tool_names_are_scoped_by_server_and_config_change_clears_catalogs() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let registry = Registry::open(&root.path().join("state")).unwrap();
    let doc = json!({"version":1,"default_profile":"default","profiles":{"default":{"servers":{"first":server(&a,"normal"),"second":server(&b,"normal")}}}});
    registry
        .replace(Document::parse(doc.to_string().as_bytes()).unwrap())
        .unwrap();
    let ctx = context();
    let mut m = Manager::new(&registry, None, None, ceiling(), &ctx).unwrap();
    for server in ["first", "second"] {
        connect(&mut m, server, &ctx);
        assert_eq!(
            m.discover(server, Kind::Tools, &ctx, |p| p.permit())
                .unwrap()
                .server,
            server
        );
    }
    let first = m.catalog("first", Kind::Tools).unwrap().unwrap().items[0]
        .identity()
        .to_owned();
    let second = m.catalog("second", Kind::Tools).unwrap().unwrap().items[0]
        .identity()
        .to_owned();
    assert_eq!(first, second);
    registry.edit_server(None, "first", Some(false)).unwrap();
    assert!(m.catalog("first", Kind::Tools).unwrap().is_none());
    assert!(m.catalog("second", Kind::Tools).unwrap().is_none());
    assert!(m.statuses().is_empty());
}
#[test]
fn shared_catalog_budget_rejects_growth_without_evicting_other_valid_servers() {
    let root = tempfile::tempdir().unwrap();
    let registry = Registry::open(&root.path().join("state")).unwrap();
    let mut servers = serde_json::Map::new();
    for n in 0..5 {
        let work = root.path().join(format!("s{n}"));
        std::fs::create_dir(&work).unwrap();
        servers.insert(format!("s{n}"), server(&work, "large"));
    }
    let doc =
        json!({"version":1,"default_profile":"default","profiles":{"default":{"servers":servers}}});
    registry
        .replace(Document::parse(doc.to_string().as_bytes()).unwrap())
        .unwrap();
    let ctx = context();
    let mut m = Manager::new(&registry, None, None, ceiling(), &ctx).unwrap();
    for n in 0..5 {
        let name = format!("s{n}");
        connect(&mut m, &name, &ctx);
        let result = m.discover(&name, Kind::Tools, &ctx, |p| p.permit());
        if n < 4 {
            assert_eq!(result.unwrap().items.len(), 4);
        } else {
            assert!(matches!(
                result,
                Err(Error::Discovery(DiscoveryError::Limit))
            ));
            assert!(m.catalog(&name, Kind::Tools).unwrap().is_none());
        }
    }
    assert!(m.catalog("s0", Kind::Tools).unwrap().is_some());
    m.close(&ctx).unwrap();
}
