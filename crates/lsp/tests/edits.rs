use namzu_core::{Cancellation, TurnContext};
use namzu_lsp::*;
use serde_json::json;
use std::{fs, time::Duration};
fn ctx() -> TurnContext {
    TurnContext::new(Duration::from_secs(30), Cancellation::default()).unwrap()
}
fn p(line: u32, character: u32) -> Position {
    Position { line, character }
}
fn edit(start: Position, end: Position, text: &str) -> TextEdit {
    TextEdit {
        range: Range { start, end },
        new_text: text.into(),
    }
}
fn setup() -> (tempfile::TempDir, Workspace, Document) {
    let root = tempfile::tempdir().unwrap();
    let text = "let value = \"🌍\";\r\nvalue;\n";
    fs::write(root.path().join("code.rs"), text).unwrap();
    let w = Workspace::new(root.path()).unwrap();
    let d = Document {
        uri: w.uri("code.rs").unwrap(),
        version: 1,
        sha256: digest(text),
        text: text.into(),
        language: "rust".into(),
    };
    (root, w, d)
}
#[test]
fn utf16_surrogates_crlf_eof_and_outside_positions() {
    let text = "a🌍b\r\nlast\n";
    assert_eq!(offset(text, p(0, 1)).unwrap(), 1);
    assert_eq!(offset(text, p(0, 3)).unwrap(), 5);
    assert!(offset(text, p(0, 2)).is_err());
    assert_eq!(offset(text, p(0, 4)).unwrap(), 6);
    assert!(offset(text, p(0, 5)).is_err());
    assert_eq!(offset(text, p(2, 0)).unwrap(), text.len());
    assert!(offset(text, p(3, 0)).is_err());
    assert_eq!(offset("a\rb\r\nc\n", p(1, 1)).unwrap(), 3);
    assert_eq!(offset("a\rb\r\nc\n", p(2, 0)).unwrap(), 5);
}
#[test]
fn multiple_edits_use_original_coordinates_and_preserve_untouched_unicode() {
    let (_, _, d) = setup();
    let edits = [
        edit(p(1, 0), p(1, 5), "renamed"),
        edit(p(0, 4), p(0, 9), "renamed"),
    ];
    assert_eq!(
        apply_text(&d.text, &edits).unwrap(),
        "let renamed = \"🌍\";\r\nrenamed;\n"
    );
}
#[test]
fn overlapping_ambiguous_reversed_and_oversized_edits_are_rejected() {
    let (_, _, d) = setup();
    for edits in [
        vec![edit(p(0, 4), p(0, 9), "x"), edit(p(0, 5), p(0, 6), "y")],
        vec![edit(p(0, 4), p(0, 4), "x"), edit(p(0, 4), p(0, 4), "y")],
        vec![edit(p(1, 5), p(1, 0), "x")],
        vec![edit(p(0, 0), p(0, 0), &"x".repeat(DOCUMENT_BYTES + 1))],
        vec![],
    ] {
        assert!(apply_text(&d.text, &edits).is_err());
    }
}
#[test]
fn workspace_edit_versions_and_resource_operations_are_checked_before_effects() {
    let (root, _, d) = setup();
    let changes = json!({"changes":{d.uri.clone():[edit(p(0,4),p(0,9),"x")]}});
    let plan = plan_from_edit(&d, &changes).unwrap();
    assert_eq!(plan.expected_sha256, d.sha256);
    for value in [
        json!({"changes":{d.uri.clone():[],"file:///tmp/other":[]}}),
        json!({"documentChanges":[{"kind":"delete","uri":d.uri}]}),
        json!({"documentChanges":[{"textDocument":{"uri":d.uri,"version":2},"edits":[]}]}),
        json!({"changes":{},"documentChanges":[]}),
    ] {
        assert!(plan_from_edit(&d, &value).is_err());
    }
    assert_eq!(
        fs::read_to_string(root.path().join("code.rs")).unwrap(),
        d.text
    );
}
#[test]
fn hash_bound_atomic_apply_preserves_mode_and_rejects_replay() {
    let (root, w, d) = setup();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            root.path().join("code.rs"),
            fs::Permissions::from_mode(0o750),
        )
        .unwrap();
    }
    let plan = EditPlan {
        uri: d.uri,
        version: 1,
        expected_sha256: d.sha256,
        edits: vec![edit(p(0, 4), p(0, 9), "new_value")],
    };
    let applied = w.apply(&plan, &ctx()).unwrap();
    assert_eq!(
        applied.sha256,
        digest(&fs::read_to_string(root.path().join("code.rs")).unwrap())
    );
    assert!(w.apply(&plan, &ctx()).is_err());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(root.path().join("code.rs"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o750
        );
    }
}
#[test]
fn changed_source_and_cancel_leave_no_temporary_or_partial_edit() {
    let (root, w, d) = setup();
    let plan = EditPlan {
        uri: d.uri,
        version: 1,
        expected_sha256: d.sha256,
        edits: vec![edit(p(0, 4), p(0, 9), "x")],
    };
    fs::write(root.path().join("code.rs"), "operator changed").unwrap();
    assert!(w.apply(&plan, &ctx()).is_err());
    assert_eq!(
        fs::read_to_string(root.path().join("code.rs")).unwrap(),
        "operator changed"
    );
    let c = ctx();
    c.cancellation().cancel();
    assert!(w.apply(&plan, &c).is_err());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}
#[test]
fn paths_uri_authorities_symlinks_and_hardlinks_are_confined() {
    let (root, w, mut d) = setup();
    for path in ["../x", "/etc/passwd", "a\0b"] {
        assert!(w.read(path, &ctx()).is_err());
        assert!(w.uri(path).is_err());
    }
    for uri in [
        "https://example.com/a",
        "file://remote/tmp/code.rs",
        "file:///tmp/outside.rs",
        "file:///tmp/x?q=a",
    ] {
        let plan = EditPlan {
            uri: uri.into(),
            version: 1,
            expected_sha256: d.sha256.clone(),
            edits: vec![edit(p(0, 4), p(0, 9), "x")],
        };
        assert!(w.apply(&plan, &ctx()).is_err());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("private"), "private").unwrap();
        symlink(outside.path(), root.path().join("escape")).unwrap();
        assert!(w.read("escape/private", &ctx()).is_err());
        symlink("code.rs", root.path().join("link.rs")).unwrap();
        d.uri = w.uri("link.rs").unwrap();
        let plan = EditPlan {
            uri: d.uri,
            version: 1,
            expected_sha256: d.sha256.clone(),
            edits: vec![edit(p(0, 4), p(0, 9), "x")],
        };
        assert!(w.apply(&plan, &ctx()).is_err());
        fs::hard_link(root.path().join("code.rs"), root.path().join("hard.rs")).unwrap();
        let plan = EditPlan {
            uri: w.uri("code.rs").unwrap(),
            version: 1,
            expected_sha256: d.sha256,
            edits: plan.edits,
        };
        assert!(w.apply(&plan, &ctx()).is_err());
    }
}
#[test]
fn source_size_utf8_and_special_files_have_bounded_failures() {
    let (root, w, _) = setup();
    let f = fs::File::create(root.path().join("large")).unwrap();
    f.set_len(DOCUMENT_BYTES as u64 + 1).unwrap();
    assert!(w.read("large", &ctx()).is_err());
    fs::write(root.path().join("binary"), [255]).unwrap();
    assert!(w.read("binary", &ctx()).is_err());
    assert!(w.read(".", &ctx()).is_err());
    #[cfg(unix)]
    {
        assert!(
            std::process::Command::new("mkfifo")
                .arg(root.path().join("fifo"))
                .status()
                .unwrap()
                .success()
        );
        assert!(w.read("fifo", &ctx()).is_err());
    }
}
