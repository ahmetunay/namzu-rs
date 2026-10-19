#![cfg(unix)]
use serde_json::{Value, json};
use std::{fs, process::Command};
fn fixture() -> String {
    format!(
        "{}/../lsp/tests/fixtures/server.py",
        env!("CARGO_MANIFEST_DIR")
    )
}
fn command(root: &std::path::Path, args: &[&str]) -> Value {
    let result = Command::new(env!("CARGO_BIN_EXE_namzu-rs"))
        .args(["lsp", "--root"])
        .arg(root)
        .args(["--program", "/usr/bin/python3", "--server-arg", &fixture()])
        .args(args)
        .env("NAMZU_LSP_ENV_SECRET_FIXTURE", "must-not-inherit")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_slice(&result.stdout).unwrap()
}
#[test]
fn explicit_cli_lsp_diagnostics_and_rename_preview_leave_source_unchanged() {
    let root = tempfile::tempdir().unwrap();
    let text = "let value = 1;\nvalue;\n";
    fs::write(root.path().join("code.rs"), text).unwrap();
    let d = command(root.path(), &["diagnostics", "code.rs"]);
    assert_eq!(d["output"]["result"]["diagnostics"][0]["source"], "toy-lsp");
    let r = command(
        root.path(),
        &[
            "rename",
            "code.rs",
            "--line",
            "1",
            "--character",
            "0",
            "--new-name",
            "next",
        ],
    );
    assert_eq!(r["output"]["result"]["preview"], "let next = 1;\nnext;\n");
    assert_eq!(r["output"]["result"]["applied"], false);
    assert_eq!(
        fs::read_to_string(root.path().join("code.rs")).unwrap(),
        text
    );
    assert_eq!(r["output"]["close"]["exit_code"], 0);
}
#[test]
fn offline_plan_apply_needs_write_flag_and_hash_then_rejects_replay() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("code.rs"), "let value = 1;\nvalue;\n").unwrap();
    let r = command(
        root.path(),
        &[
            "rename",
            "code.rs",
            "--line",
            "1",
            "--character",
            "0",
            "--new-name",
            "next",
        ],
    );
    fs::write(
        root.path().join("plan.json"),
        serde_json::to_vec(&r["output"]["result"]["plan"]).unwrap(),
    )
    .unwrap();
    for (write, success) in [(false, false), (true, true), (true, false)] {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_namzu-rs"));
        cmd.args(["lsp", "--root"])
            .arg(root.path())
            .args(["apply", "plan.json"]);
        if write {
            cmd.arg("--allow-write");
        }
        let result = cmd.output().unwrap();
        assert_eq!(result.status.success(), success);
    }
    assert_eq!(
        fs::read_to_string(root.path().join("code.rs")).unwrap(),
        "let next = 1;\nnext;\n"
    );
}
#[test]
fn invalid_source_position_is_rejected_before_server_spawn() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("code.rs"), "🌍").unwrap();
    let marker = root.path().join("spawned");
    let result = Command::new(env!("CARGO_BIN_EXE_namzu-rs"))
        .args(["lsp", "--root"])
        .arg(root.path())
        .args(["--program", "/bin/sh", "--server-arg", "-c", "--server-arg"])
        .arg(format!("touch {}", marker.display()))
        .args(["definition", "code.rs", "--line", "0", "--character", "1"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!marker.exists());
    let value: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["status"], json!("error"));
}
