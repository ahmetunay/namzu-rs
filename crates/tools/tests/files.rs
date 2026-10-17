use namzu_core::*;
use namzu_tools::FileTools;
use serde_json::json;
use tempfile::TempDir;

fn perform(root: &TempDir, writable: bool, name: &str, args: serde_json::Value) -> ToolResult {
    let mut tools = FileTools::new(root.path(), writable).unwrap();
    let mut provider = ScriptedProvider(
        vec![
            Completion {
                text: "".into(),
                tool_calls: vec![ToolCall {
                    id: "test".into(),
                    name: name.into(),
                    arguments: args,
                }],
            },
            Completion {
                text: "done".into(),
                tool_calls: vec![],
            },
        ]
        .into(),
    );
    let turn = run_turn(
        &mut provider,
        &mut tools,
        "test",
        Limits::default(),
        Cancellation::default(),
    )
    .unwrap();
    match &turn.messages[2] {
        Message::Tool {
            content, is_error, ..
        } => ToolResult {
            content: content.clone(),
            is_error: *is_error,
        },
        _ => panic!("missing tool output"),
    }
}

#[test]
fn write_then_read_exact_utf8() {
    let root = TempDir::new().unwrap();
    assert!(
        !perform(
            &root,
            true,
            "write_file",
            json!({"path":"hello.txt","content":"merhaba 🌍\n"})
        )
        .is_error
    );
    assert_eq!(
        perform(&root, false, "read_file", json!({"path":"hello.txt"})),
        ToolResult::ok("merhaba 🌍\n")
    );
}

#[test]
fn readonly_never_advertises_or_executes_writes() {
    let root = TempDir::new().unwrap();
    let tools = FileTools::new(root.path(), false).unwrap();
    assert!(!tools.definitions().iter().any(|d| d.name == "write_file"));
    assert!(
        perform(
            &root,
            false,
            "write_file",
            json!({"path":"x","content":"x"})
        )
        .is_error
    );
    assert!(!root.path().join("x").exists());
}

#[test]
fn absolute_and_parent_paths_are_refused() {
    let root = TempDir::new().unwrap();
    for path in ["../outside", "a/../../outside", "/etc/passwd", ""] {
        assert!(perform(&root, true, "read_file", json!({"path":path})).is_error);
        assert!(
            perform(
                &root,
                true,
                "write_file",
                json!({"path":path,"content":"changed"})
            )
            .is_error
        );
    }
}

#[cfg(unix)]
#[test]
fn escaping_symlink_cannot_read_or_write_external_file() {
    use std::os::unix::fs::symlink;
    let root = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    std::fs::write(outside.path().join("value"), "original").unwrap();
    symlink(outside.path(), root.path().join("escape")).unwrap();
    assert!(perform(&root, true, "read_file", json!({"path":"escape/value"})).is_error);
    assert!(
        perform(
            &root,
            true,
            "write_file",
            json!({"path":"escape/value","content":"corrupt"})
        )
        .is_error
    );
    assert_eq!(
        std::fs::read_to_string(outside.path().join("value")).unwrap(),
        "original"
    );
}

#[cfg(unix)]
#[test]
fn replacing_leaf_symlink_leaves_external_target_unchanged() {
    use std::os::unix::fs::symlink;
    let root = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let target = outside.path().join("value");
    std::fs::write(&target, "original").unwrap();
    symlink(&target, root.path().join("leaf")).unwrap();
    assert!(
        !perform(
            &root,
            true,
            "write_file",
            json!({"path":"leaf","content":"local"})
        )
        .is_error
    );
    assert_eq!(std::fs::read_to_string(target).unwrap(), "original");
    assert_eq!(
        std::fs::read_to_string(root.path().join("leaf")).unwrap(),
        "local"
    );
}

#[test]
fn listing_is_sorted_and_bounded() {
    let root = TempDir::new().unwrap();
    for name in ["b", "a"] {
        std::fs::write(root.path().join(name), "").unwrap();
    }
    assert_eq!(
        perform(&root, false, "list_files", json!({"path":"."})),
        ToolResult::ok("[\"a\",\"b\"]")
    );
    for i in 0..512 {
        std::fs::write(root.path().join(format!("entry-{i}")), "").unwrap();
    }
    assert!(perform(&root, false, "list_files", json!({"path":"."})).is_error);
}

#[test]
fn oversized_files_and_writes_are_rejected_without_replacement() {
    let root = TempDir::new().unwrap();
    let large = "x".repeat(65537);
    std::fs::write(root.path().join("large"), &large).unwrap();
    std::fs::write(root.path().join("keep"), "original").unwrap();
    assert!(perform(&root, false, "read_file", json!({"path":"large"})).is_error);
    assert!(
        perform(
            &root,
            true,
            "write_file",
            json!({"path":"keep","content":large})
        )
        .is_error
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("keep")).unwrap(),
        "original"
    );
}

#[test]
fn unexpected_fields_are_rejected_before_write() {
    let root = TempDir::new().unwrap();
    assert!(
        perform(
            &root,
            true,
            "write_file",
            json!({"path":"x","content":"x","append":true})
        )
        .is_error
    );
    assert!(!root.path().join("x").exists());
}

#[cfg(unix)]
#[test]
fn fifo_is_rejected_without_waiting_for_a_writer() {
    let root = TempDir::new().unwrap();
    let result = std::process::Command::new("mkfifo")
        .arg(root.path().join("pipe"))
        .status()
        .unwrap();
    assert!(result.success());
    assert!(perform(&root, false, "read_file", json!({"path":"pipe"})).is_error);
}
