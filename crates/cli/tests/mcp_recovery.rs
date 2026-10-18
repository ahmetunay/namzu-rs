#[test]
fn real_cli_reconnect_subscription_and_durable_no_replay() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/mcp_recovery_smoke.py");
    let out = std::process::Command::new("python3")
        .arg(script)
        .args(["--binary", env!("CARGO_BIN_EXE_namzu-rs")])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["checks"].as_array().unwrap().len(), 5);
    assert_eq!(report["live_model_calls"], 0);
}
