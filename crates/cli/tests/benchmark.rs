use serde_json::Value;
use std::process::Command;

#[test]
fn demo_runs_three_independent_oracles_and_detects_noop_lies() {
    let out = tempfile::tempdir().unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_namzu-rs"))
        .args(["bench", "demo", "--out", out.path().to_str().unwrap()])
        .env_remove("OPENAI_API_KEY")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["mode"], "fixture");
    assert_eq!(report["timeout_seconds"], 300);
    assert_eq!(report["summary"][0]["passed"], 3);
    assert_eq!(report["summary"][1]["passed"], 0);
    assert_eq!(report["runs"].as_array().unwrap().len(), 6);
}
