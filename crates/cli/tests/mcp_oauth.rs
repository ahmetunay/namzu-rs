#[test]
fn independent_cli_oauth_pkce_refresh_agent_and_cancellation() {
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/mcp_oauth_smoke.py");
    let output = std::process::Command::new("python3")
        .arg(script)
        .args(["--binary", env!("CARGO_BIN_EXE_namzu-rs")])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["live_model_calls"], 0);
    assert_eq!(report["checks"].as_array().unwrap().len(), 11);
}
