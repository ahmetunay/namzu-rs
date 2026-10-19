use serde_json::Value;
use std::{fs, process::Command};
fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_namzu-rs"))
}
#[test]
fn offline_inspection_needs_no_speech_credentials_and_reports_exact_pcm() {
    let root = tempfile::tempdir().unwrap();
    let audio = namzu_audio::Wav::pcm16(16000, 1, &vec![0; 1600]).unwrap();
    fs::write(root.path().join("input.wav"), audio.bytes()).unwrap();
    let result = binary()
        .args(["audio", "--cwd"])
        .arg(root.path())
        .args(["inspect", "input.wav"])
        .env_remove("OPENAI_API_KEY")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let value: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["output"]["offline"], true);
    assert_eq!(value["output"]["audio"]["frames"], 1600);
    assert_eq!(value["output"]["audio"]["sha256"], audio.info().sha256);
}
#[test]
fn invalid_input_is_rejected_before_key_or_network_and_is_not_echoed() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("bad.wav"), "private source not audio").unwrap();
    let result = binary()
        .args(["audio", "--cwd"])
        .arg(root.path())
        .args([
            "transcribe",
            "bad.wav",
            "--model",
            "fixture",
            "--api-key-env",
            "NAMZU_AUDIO_TEST_MISSING_KEY_52",
        ])
        .env_remove("NAMZU_AUDIO_TEST_MISSING_KEY_52")
        .output()
        .unwrap();
    assert!(!result.status.success());
    let value: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(value["error"].as_str().unwrap().contains("WAV"));
    assert!(!String::from_utf8_lossy(&result.stdout).contains("private source"));
}
#[test]
fn destination_preflight_prevents_speech_charge_and_preserves_operator_file() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("out.wav"), "operator-owned").unwrap();
    let result = binary()
        .args(["audio", "--cwd"])
        .arg(root.path())
        .args([
            "synthesize",
            "test",
            "--output",
            "out.wav",
            "--voice",
            "alloy",
            "--model",
            "fixture",
            "--api-key-env",
            "NAMZU_AUDIO_TEST_MISSING_KEY_52",
        ])
        .env_remove("NAMZU_AUDIO_TEST_MISSING_KEY_52")
        .output()
        .unwrap();
    assert!(!result.status.success());
    let value: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(
        value["error"]
            .as_str()
            .unwrap()
            .contains("destination exists")
    );
    assert_eq!(
        fs::read(root.path().join("out.wav")).unwrap(),
        b"operator-owned"
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}
