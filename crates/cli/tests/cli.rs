use serde_json::Value;
use std::process::Command;

#[test]
fn offline_demo_satisfies_filesystem_oracle_and_json_contract() {
    let output = Command::new(env!("CARGO_BIN_EXE_namzu-rs"))
        .args(["demo", "--json"])
        .env_remove("OPENAI_API_KEY")
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["status"], "ok");
    assert_eq!(value["output"], "12");
    assert_eq!(value["offline"], true);
    assert_eq!(value["stats"]["provider_calls"], 3);
    assert_eq!(value["stats"]["tool_calls"], 2);
}

#[test]
fn missing_key_fails_as_json_without_network_access() {
    let output = Command::new(env!("CARGO_BIN_EXE_namzu-rs"))
        .args([
            "exec",
            "test",
            "--model",
            "fixture",
            "--json",
            "--api-key-env",
            "NAMZU_TEST_MISSING_KEY",
        ])
        .env_remove("NAMZU_TEST_MISSING_KEY")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["status"], "error");
    assert_eq!(value["offline"], false);
    assert!(value["error"].as_str().unwrap().contains("missing"));
}

#[test]
fn invalid_timeout_fails_before_key_or_network_access() {
    let output = Command::new(env!("CARGO_BIN_EXE_namzu-rs"))
        .args(["exec", "test", "--model", "fixture", "--timeout", "0"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid value"));
}
