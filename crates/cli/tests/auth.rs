use serde_json::Value;
use std::process::Command;

#[test]
fn auth_status_and_logout_do_not_discover_other_harness_credentials() {
    let root = tempfile::tempdir().unwrap();
    for action in ["status", "logout", "status"] {
        let result = Command::new(env!("CARGO_BIN_EXE_namzu-rs"))
            .args(["auth", "--state-dir", root.path().to_str().unwrap(), action])
            .output()
            .unwrap();
        assert!(result.status.success());
        let value: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(value["authenticated"], false);
    }
}

#[test]
fn codex_missing_login_fails_before_inference_without_api_key_fallback() {
    let root = tempfile::tempdir().unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_namzu-rs"))
        .args([
            "exec",
            "test",
            "--provider",
            "codex",
            "--model",
            "fixture",
            "--state-dir",
            root.path().to_str().unwrap(),
            "--json",
        ])
        .env("OPENAI_API_KEY", "never-used-fixture")
        .output()
        .unwrap();
    assert!(!result.status.success());
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(text.contains("login required"));
    assert!(!text.contains("never-used-fixture"));
}

#[test]
fn codex_never_sends_subscription_tokens_to_a_base_url_override() {
    let root = tempfile::tempdir().unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_namzu-rs"))
        .args([
            "exec",
            "test",
            "--provider",
            "codex",
            "--model",
            "fixture",
            "--state-dir",
            root.path().to_str().unwrap(),
            "--base-url",
            "https://example.com",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(!result.status.success());
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(text.contains("fixed subscription endpoint"));
}
