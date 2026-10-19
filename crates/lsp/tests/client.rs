#![cfg(unix)]
use namzu_core::{AgentError, Cancellation, TurnContext};
use namzu_lsp::*;
use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};
fn ctx() -> TurnContext {
    TurnContext::new(Duration::from_secs(20), Cancellation::default()).unwrap()
}
fn start(root: &Path, mode: &str, c: &TurnContext) -> Result<Client, AgentError> {
    let mut spec = Spec::new("/usr/bin/python3", root);
    spec.args = vec![
        format!("{}/tests/fixtures/server.py", env!("CARGO_MANIFEST_DIR")),
        "--mode".into(),
        mode.into(),
        "--marker".into(),
        root.join("marker").to_str().unwrap().into(),
    ];
    Client::start(spec, c)
}
fn source(root: &Path) {
    fs::write(root.join("code.rs"), "let value = \"🌍\";\nvalue;\n").unwrap();
}
#[test]
fn real_stdio_definition_references_rename_change_close_and_shutdown() {
    let root = tempfile::tempdir().unwrap();
    source(root.path());
    let c = ctx();
    let mut client = start(root.path(), "normal", &c).unwrap();
    let doc = client.open("code.rs", "rust", &c).unwrap();
    let at = Position {
        line: 1,
        character: 0,
    };
    assert_eq!(
        client.definition(&doc.uri, at, &c).unwrap()[0].range.start,
        Position {
            line: 0,
            character: 4
        }
    );
    assert_eq!(client.references(&doc.uri, at, &c).unwrap().len(), 2);
    assert_eq!(
        client.diagnostics(&doc.uri, &c).unwrap()[0].severity,
        Some(2)
    );
    let plan = client.rename(&doc.uri, at, "next_value", &c).unwrap();
    assert_eq!(
        fs::read_to_string(root.path().join("code.rs")).unwrap(),
        doc.text
    );
    client.apply(&plan, &c).unwrap();
    assert!(client.apply(&plan, &c).is_err());
    assert_eq!(
        fs::read_to_string(root.path().join("code.rs")).unwrap(),
        "let next_value = \"🌍\";\nnext_value;\n"
    );
    client.close_document(&doc.uri, &c).unwrap();
    let report = client.close(&ctx());
    assert_eq!(report.exit_code, Some(0));
    assert!(!report.forced);
}
#[test]
fn fragmented_frames_pull_reports_stale_push_and_location_links_interoperate() {
    for mode in ["fragment", "pull", "stale", "link", "server_requests"] {
        let root = tempfile::tempdir().unwrap();
        source(root.path());
        let c = ctx();
        let mut client = start(root.path(), mode, &c).unwrap();
        let doc = client.open("code.rs", "rust", &c).unwrap();
        assert_eq!(
            client.diagnostics(&doc.uri, &c).unwrap()[0].message,
            "Untrusted fixture diagnostic"
        );
        if mode == "stale" {
            let plan = client
                .rename(
                    &doc.uri,
                    Position {
                        line: 1,
                        character: 0,
                    },
                    "other",
                    &c,
                )
                .unwrap();
            client.apply(&plan, &c).unwrap();
            assert_eq!(
                client.diagnostics(&doc.uri, &c).unwrap()[0].message,
                "Untrusted fixture diagnostic"
            );
        }
        assert_eq!(
            client
                .definition(
                    &doc.uri,
                    Position {
                        line: 1,
                        character: 0
                    },
                    &c
                )
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            fs::read_to_string(root.path().join("code.rs")).unwrap(),
            if mode == "stale" {
                doc.text.replace("value", "other")
            } else {
                doc.text
            }
        );
        assert_eq!(client.close(&ctx()).exit_code, Some(0));
    }
}
#[test]
fn bad_encoding_ids_partial_frames_oversize_and_stderr_fail_closed() {
    for mode in ["utf8", "wrong_id", "partial", "oversize", "stderr"] {
        let root = tempfile::tempdir().unwrap();
        let c = ctx();
        assert!(start(root.path(), mode, &c).is_err(), "{mode}");
    }
}
#[test]
fn cross_workspace_locations_and_multi_file_or_stale_edits_never_write() {
    for mode in ["outside", "multi", "version"] {
        let root = tempfile::tempdir().unwrap();
        source(root.path());
        let c = ctx();
        let mut client = start(root.path(), mode, &c).unwrap();
        let doc = client.open("code.rs", "rust", &c).unwrap();
        let at = Position {
            line: 1,
            character: 0,
        };
        if mode == "outside" {
            assert!(client.definition(&doc.uri, at, &c).is_err());
        } else {
            assert!(client.rename(&doc.uri, at, "changed", &c).is_err());
        }
        assert_eq!(
            fs::read_to_string(root.path().join("code.rs")).unwrap(),
            doc.text
        );
        client.close(&ctx());
    }
}
#[test]
fn cancellation_after_admission_kills_and_does_not_restart_server() {
    let root = tempfile::tempdir().unwrap();
    source(root.path());
    let c = ctx();
    let mut client = start(root.path(), "stall", &c).unwrap();
    let doc = client.open("code.rs", "rust", &c).unwrap();
    let marker = root.path().join("marker");
    let signal = c.cancellation();
    let watcher = thread::spawn(move || {
        let end = Instant::now() + Duration::from_secs(5);
        while !marker.exists() {
            assert!(Instant::now() < end);
            thread::sleep(Duration::from_millis(2));
        }
        signal.cancel();
    });
    assert_eq!(
        client
            .definition(
                &doc.uri,
                Position {
                    line: 1,
                    character: 0
                },
                &c
            )
            .unwrap_err(),
        AgentError::Cancelled
    );
    watcher.join().unwrap();
    assert!(
        client
            .definition(
                &doc.uri,
                Position {
                    line: 1,
                    character: 0
                },
                &ctx()
            )
            .is_err()
    );
    assert!(client.close(&ctx()).forced);
}
#[test]
fn parent_deadline_applies_and_fresh_context_cannot_extend_live_session() {
    let root = tempfile::tempdir().unwrap();
    source(root.path());
    let c = ctx();
    let mut client = start(root.path(), "normal", &c).unwrap();
    assert!(client.open("code.rs", "rust", &ctx()).is_err());
    let doc = client.open("code.rs", "rust", &c).unwrap();
    assert_eq!(
        client
            .references(
                &doc.uri,
                Position {
                    line: 1,
                    character: 0
                },
                &c
            )
            .unwrap()
            .len(),
        2
    );
    client.close(&ctx());
}
#[test]
fn graceful_leader_shutdown_also_kills_pipe_inheriting_descendant() {
    let root = tempfile::tempdir().unwrap();
    let c = ctx();
    let mut client = start(root.path(), "descendant", &c).unwrap();
    let pid: i32 = fs::read_to_string(root.path().join("marker"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(client.close(&ctx()).exit_code, Some(0));
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        let state = fs::read_to_string(format!("/proc/{pid}/stat")).ok();
        if state
            .as_ref()
            .is_none_or(|s| s.split_whitespace().nth(2) == Some("Z"))
        {
            break;
        }
        assert!(Instant::now() < end, "descendant still running");
        thread::sleep(Duration::from_millis(2));
    }
}
