use std::process::Command;

#[test]
fn tui_refuses_piped_input_before_creating_state_or_requesting_a_model() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("uncreated-state");
    let output = Command::new(env!("CARGO_BIN_EXE_namzu-rs"))
        .args([
            "tui",
            "--provider",
            "codex",
            "--model",
            "fixture",
            "--state-dir",
        ])
        .arg(&state)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("TUI requires a terminal"));
    assert!(output.stdout.is_empty() && !state.exists());
}

#[test]
fn tui_help_exposes_explicit_permissions_and_offline_demo_without_json_ambiguity() {
    let output = Command::new(env!("CARGO_BIN_EXE_namzu-rs"))
        .args(["tui", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("--demo") && help.contains("--allow-shell") && help.contains("--resume"));
    assert!(!help.contains("--json"));
}

#[test]
fn selected_conversation_byte_budget_stops_before_any_http_request() {
    let root = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_namzu-rs"))
        .args([
            "exec",
            "too large",
            "--model",
            "fixture",
            "--base-url",
            "http://127.0.0.1:1/v1",
            "--api-key-env",
            "NAMZU_FIXTURE_KEY",
            "--max-text-bytes",
            "1",
            "--json",
            "--state-dir",
        ])
        .arg(root.path().join("state"))
        .env("NAMZU_FIXTURE_KEY", "fixture-only")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        value["error"]
            .as_str()
            .unwrap()
            .contains("conversation exceeds byte budget")
    );
}
