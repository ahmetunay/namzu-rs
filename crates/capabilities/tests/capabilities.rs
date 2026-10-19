use namzu_capabilities::{AudioConfig, CapabilityTools, Config, LspConfig};
use namzu_core::{
    Cancellation, Completion, Limits, ScriptedProvider, ToolCall, Tools, TurnContext, run_turn,
};
use namzu_tools::{TerminalTools, ToolPolicy};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
fn ctx() -> TurnContext {
    TurnContext::new(Duration::from_secs(20), Cancellation::default()).unwrap()
}
fn selected(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}
fn call(name: &str, args: Value) -> ToolCall {
    ToolCall {
        id: name.into(),
        name: name.into(),
        arguments: args,
    }
}
fn config() -> Config {
    Config {
        audio: Some(AudioConfig {
            base_url: "https://example.invalid/v1".into(),
            api_key_env: "FIXTURE_KEY".into(),
            transcription_model: "fixture-stt".into(),
            speech_model: "fixture-tts".into(),
        }),
        ..Default::default()
    }
}
struct Speech(Arc<AtomicUsize>);
impl namzu_audio::SpeechInput for Speech {
    fn transcribe(
        &mut self,
        _: &namzu_audio::Wav,
        _: Option<&str>,
        _: &TurnContext,
    ) -> Result<namzu_audio::Transcript, namzu_core::AgentError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(namzu_audio::Transcript {
            text: "fixture transcript".into(),
            language: None,
        })
    }
}
impl namzu_audio::SpeechOutput for Speech {
    fn synthesize(
        &mut self,
        _: &str,
        _: &str,
        _: &TurnContext,
    ) -> Result<namzu_audio::Wav, namzu_core::AgentError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        namzu_audio::Wav::pcm16(8000, 1, &[0; 80])
    }
}
fn tools(
    root: &std::path::Path,
    config: Config,
    names: &[&str],
    policy: ToolPolicy,
) -> CapabilityTools<TerminalTools> {
    CapabilityTools::new(
        TerminalTools::new(root, policy.clone()).unwrap(),
        root,
        config,
        selected(names),
        policy,
    )
    .unwrap()
}
fn wav(root: &std::path::Path) {
    std::fs::write(
        root.join("in.wav"),
        namzu_audio::Wav::pcm16(8000, 1, &[0; 80]).unwrap().bytes(),
    )
    .unwrap();
}
#[test]
fn settings_never_activate_packages_or_manufacture_grants() {
    let root = tempfile::tempdir().unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let t = tools(root.path(), config(), &[], ToolPolicy::default())
        .grants(true, true)
        .with_speech(Box::new(Speech(count.clone())));
    assert!(
        t.definitions()
            .iter()
            .all(|d| !d.name.starts_with("audio_"))
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
    let t = tools(root.path(), config(), &["audio"], ToolPolicy::default());
    assert!(t.definitions().iter().any(|d| d.name == "audio_inspect"));
    assert!(t.definitions().iter().all(|d| d.name != "audio_transcribe"));
}
#[test]
fn package_and_tool_denies_intersect_with_current_grants() {
    let root = tempfile::tempdir().unwrap();
    let mut c = config();
    c.deny.insert("audio".into());
    assert!(c.identity(&selected(&["audio"])).is_err());
    let p = ToolPolicy {
        allow_write: true,
        allow: Some(selected(&["audio_synthesize"])),
        ..Default::default()
    };
    let t = tools(root.path(), config(), &["audio"], p)
        .grants(true, false)
        .with_speech(Box::new(Speech(Arc::default())));
    assert_eq!(t.definitions().len(), 1);
    assert_eq!(t.definitions()[0].name, "audio_synthesize");
}
#[test]
fn audio_roundtrip_inspect_transcript_and_new_file_speech() {
    let root = tempfile::tempdir().unwrap();
    wav(root.path());
    let count = Arc::new(AtomicUsize::new(0));
    let mut t = tools(
        root.path(),
        config(),
        &["audio"],
        ToolPolicy {
            allow_write: true,
            ..Default::default()
        },
    )
    .grants(true, false)
    .with_speech(Box::new(Speech(count.clone())));
    assert!(
        !t.call(&call("audio_inspect", json!({"path":"in.wav"})), &ctx())
            .unwrap()
            .is_error
    );
    let result = t
        .call(&call("audio_transcribe", json!({"path":"in.wav"})), &ctx())
        .unwrap();
    assert!(result.content.contains("fixture transcript"));
    let synthesis = call(
        "audio_synthesize",
        json!({"text":"hello","voice":"fixture","output":"out.wav"}),
    );
    t.call(&synthesis, &ctx()).unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert!(t.call(&synthesis, &ctx()).is_err());
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert!(namzu_audio::Wav::parse(std::fs::read(root.path().join("out.wav")).unwrap()).is_ok());
}
#[test]
fn audio_write_and_online_permissions_are_independent() {
    let root = tempfile::tempdir().unwrap();
    wav(root.path());
    let mut t = tools(root.path(), config(), &["audio"], ToolPolicy::default())
        .grants(true, false)
        .with_speech(Box::new(Speech(Arc::default())));
    assert!(t.definitions().iter().any(|d| d.name == "audio_transcribe"));
    assert!(t.definitions().iter().all(|d| d.name != "audio_synthesize"));
    assert!(
        t.call(
            &call(
                "audio_synthesize",
                json!({"text":"hello","voice":"fixture","output":"out.wav"})
            ),
            &ctx()
        )
        .is_err()
    );
}
#[test]
fn invalid_later_audio_call_prevents_first_batch_write() {
    let root = tempfile::tempdir().unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let mut t = tools(
        root.path(),
        config(),
        &["audio"],
        ToolPolicy {
            allow_write: true,
            ..Default::default()
        },
    )
    .grants(true, false)
    .with_speech(Box::new(Speech(count.clone())));
    let mut provider = ScriptedProvider(
        [Completion {
            text: String::new(),
            tool_calls: vec![
                call("write_file", json!({"path":"first","content":"bad"})),
                call("audio_transcribe", json!({"path":"missing.wav"})),
            ],
        }]
        .into(),
    );
    assert!(
        run_turn(
            &mut provider,
            &mut t,
            "fixture",
            Limits::default(),
            Cancellation::default()
        )
        .is_err()
    );
    assert!(!root.path().join("first").exists());
    assert_eq!(count.load(Ordering::SeqCst), 0);
}
#[test]
fn traversal_invalid_wav_and_cancel_do_not_reach_speech() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("bad.wav"), b"invalid").unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let mut t = tools(root.path(), config(), &["audio"], ToolPolicy::default())
        .grants(true, false)
        .with_speech(Box::new(Speech(count.clone())));
    for path in ["../outside.wav", "/etc/passwd", "bad.wav"] {
        assert!(
            t.call(&call("audio_transcribe", json!({"path":path})), &ctx())
                .is_err()
        );
    }
    let c = ctx();
    c.cancellation().cancel();
    assert!(
        t.call(&call("audio_inspect", json!({"path":"bad.wav"})), &c)
            .is_err()
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
}
#[test]
fn config_is_strict_and_identity_only_pins_selected_settings() {
    let mut c = config();
    assert!(c.identity(&BTreeSet::new()).unwrap().is_null());
    let a = c.identity(&selected(&["audio"])).unwrap();
    c.audio.as_mut().unwrap().speech_model = "changed".into();
    assert_ne!(a, c.identity(&selected(&["audio"])).unwrap());
    assert!(serde_json::from_value::<Config>(json!({"secret":"bad"})).is_err());
    assert!(c.identity(&selected(&["unknown"])).is_err());
    assert!(c.registry(&selected(&["audio"]), true).is_err());
    c.audio.as_mut().unwrap().base_url = "http://example.invalid".into();
    assert!(c.validate().is_err());
}
#[test]
fn files_package_deny_hides_and_blocks_base_tools() {
    let root = tempfile::tempdir().unwrap();
    let mut c = Config::default();
    c.deny.insert("files".into());
    let mut t = tools(
        root.path(),
        c,
        &[],
        ToolPolicy {
            allow_write: true,
            ..Default::default()
        },
    );
    assert!(t.definitions().is_empty());
    assert!(
        t.call(
            &call("write_file", json!({"path":"x","content":"x"})),
            &ctx()
        )
        .is_err()
    );
    assert!(!root.path().join("x").exists());
}
#[cfg(unix)]
fn lsp_config() -> Config {
    Config {
        lsp: Some(LspConfig {
            program: "/usr/bin/python3".into(),
            args: vec![format!(
                "{}/../lsp/tests/fixtures/server.py",
                env!("CARGO_MANIFEST_DIR")
            )],
            env: BTreeSet::new(),
        }),
        ..Default::default()
    }
}
#[cfg(unix)]
#[test]
fn lsp_needs_current_host_execution_grant() {
    let root = tempfile::tempdir().unwrap();
    let mut t = tools(root.path(), lsp_config(), &["lsp"], ToolPolicy::default());
    assert!(t.definitions().iter().all(|d| !d.name.starts_with("lsp_")));
    assert!(
        t.call(&call("lsp_diagnostics", json!({"path":"code.rs"})), &ctx())
            .is_err()
    );
}
#[cfg(unix)]
#[test]
fn real_lsp_queries_preview_apply_and_stale_replay() {
    let root = tempfile::tempdir().unwrap();
    let original = "let value = 1;\nvalue;\n";
    std::fs::write(root.path().join("code.rs"), original).unwrap();
    let mut t = tools(
        root.path(),
        lsp_config(),
        &["lsp"],
        ToolPolicy {
            allow_write: true,
            ..Default::default()
        },
    )
    .grants(false, true);
    let diagnostics = t
        .call(&call("lsp_diagnostics", json!({"path":"code.rs"})), &ctx())
        .unwrap();
    assert!(diagnostics.content.contains("toy-lsp"));
    for name in ["lsp_definition", "lsp_references"] {
        let out = t
            .call(
                &call(name, json!({"path":"code.rs","line":1,"character":0})),
                &ctx(),
            )
            .unwrap();
        assert!(out.content.contains("locations"));
    }
    let preview = t
        .call(
            &call(
                "lsp_rename_preview",
                json!({"path":"code.rs","line":1,"character":0,"new_name":"next"}),
            ),
            &ctx(),
        )
        .unwrap();
    let preview: Value = serde_json::from_str(&preview.content).unwrap();
    assert_eq!(preview["preview"], "let next = 1;\nnext;\n");
    assert_eq!(
        std::fs::read_to_string(root.path().join("code.rs")).unwrap(),
        original
    );
    let apply = call("lsp_apply", json!({"plan":preview["plan"]}));
    t.call(&apply, &ctx()).unwrap();
    assert!(t.call(&apply, &ctx()).is_err());
}
#[cfg(unix)]
#[test]
fn invalid_lsp_position_preflight_prevents_earlier_write() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("code.rs"), "let value = 1;\n").unwrap();
    let mut t = tools(
        root.path(),
        lsp_config(),
        &["lsp"],
        ToolPolicy {
            allow_write: true,
            ..Default::default()
        },
    )
    .grants(false, true);
    let mut provider = ScriptedProvider(
        [Completion {
            text: String::new(),
            tool_calls: vec![
                call("write_file", json!({"path":"first","content":"bad"})),
                call(
                    "lsp_definition",
                    json!({"path":"code.rs","line":999,"character":0}),
                ),
            ],
        }]
        .into(),
    );
    assert!(
        run_turn(
            &mut provider,
            &mut t,
            "fixture",
            Limits::default(),
            Cancellation::default()
        )
        .is_err()
    );
    assert!(!root.path().join("first").exists());
}
