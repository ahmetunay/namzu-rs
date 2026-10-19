use namzu_audio::*;
use namzu_core::{AgentError, Cancellation, TurnContext};
use std::{fs, time::Duration};
fn ctx() -> TurnContext {
    TurnContext::new(Duration::from_secs(30), Cancellation::default()).unwrap()
}
fn wav() -> Wav {
    Wav::pcm16(16000, 1, &[0, 100, -100, 32767, -32768]).unwrap()
}
fn resize(bytes: &mut [u8]) {
    let n = (bytes.len() - 8) as u32;
    bytes[4..8].copy_from_slice(&n.to_le_bytes());
}
#[test]
fn wav_preserves_signed_samples_format_frames_and_digest() {
    let a = wav();
    assert_eq!(
        &a.bytes()[44..],
        &[0, 0, 100, 0, 156, 255, 255, 127, 0, 128]
    );
    assert_eq!(a.info().frames, 5);
    assert_eq!(a.info().channels, 1);
    assert_eq!(Wav::parse(a.bytes().to_vec()).unwrap().info(), a.info());
    assert!(!format!("{a:?}").contains("32767"));
}
#[test]
fn exact_duration_limit_and_stereo_alignment_are_enforced() {
    let samples = vec![0; 8000 * MAX_SECONDS as usize];
    let a = Wav::pcm16(8000, 1, &samples).unwrap();
    assert_eq!(a.info().duration_ms, 60000);
    let mut too_long = samples;
    too_long.push(0);
    assert!(Wav::pcm16(8000, 1, &too_long).is_err());
    assert!(Wav::pcm16(8000, 2, &[1, 2, 3]).is_err());
    assert!(Wav::pcm16(8000, 2, &[1, 2]).is_ok());
}
#[test]
fn corrupt_headers_sizes_rates_and_byte_alignment_are_rejected() {
    let good = wav().bytes().to_vec();
    for offset in [0, 4, 8, 12, 16, 20, 22, 24, 28, 32, 34, 36, 40] {
        let mut bytes = good.clone();
        bytes[offset] ^= 255;
        assert!(Wav::parse(bytes).is_err(), "offset {offset}");
    }
    let mut trailing = good.clone();
    trailing.push(0);
    assert!(Wav::parse(trailing).is_err());
    for len in 0..44 {
        assert!(Wav::parse(good[..len].to_vec()).is_err());
    }
    assert!(Wav::pcm16(7999, 1, &[1]).is_err());
    assert!(Wav::pcm16(48001, 1, &[1]).is_err());
    assert!(Wav::pcm16(16000, 0, &[1]).is_err());
    assert!(Wav::pcm16(16000, 1, &[]).is_err());
}
#[test]
fn riff_padding_metadata_duplicate_chunks_and_chunk_count() {
    let good = wav().bytes().to_vec();
    let mut metadata = good[..12].to_vec();
    metadata.extend(b"JUNK\x01\0\0\0x\0");
    metadata.extend(&good[12..]);
    resize(&mut metadata);
    assert!(Wav::parse(metadata).is_ok());
    let mut duplicate = good.clone();
    duplicate.extend(&good[36..]);
    resize(&mut duplicate);
    assert!(Wav::parse(duplicate).is_err());
    let mut duplicate = good.clone();
    duplicate.extend(&good[12..36]);
    resize(&mut duplicate);
    assert!(Wav::parse(duplicate).is_err());
    let mut too_many = good.clone();
    for _ in 0..128 {
        too_many.extend(b"JUNK\0\0\0\0");
    }
    resize(&mut too_many);
    assert!(Wav::parse(too_many).is_err());
}
#[test]
fn oversized_and_unknown_encoding_never_enter_sdk() {
    assert!(Wav::parse(vec![0; MAX_AUDIO_BYTES + 1]).is_err());
    for marker in [b"RF64", b"RIFX", b"fLaC", b"OggS"] {
        let mut bytes = wav().bytes().to_vec();
        bytes[..4].copy_from_slice(marker);
        assert!(Wav::parse(bytes).is_err());
    }
    let mut ieee = wav().bytes().to_vec();
    ieee[20] = 3;
    assert!(Wav::parse(ieee).is_err());
}
struct Fixture {
    calls: usize,
    fail: bool,
    invalid: bool,
    cancel: bool,
}
impl SpeechInput for Fixture {
    fn transcribe(
        &mut self,
        _: &Wav,
        _: Option<&str>,
        c: &TurnContext,
    ) -> Result<Transcript, AgentError> {
        self.calls += 1;
        if self.cancel {
            c.cancellation().cancel();
        }
        if self.fail {
            return Err(AgentError::Provider("fixture".into()));
        }
        Ok(Transcript {
            text: if self.invalid {
                "x".repeat(MAX_TRANSCRIPT_BYTES + 1)
            } else {
                "Türkçe özel metin".into()
            },
            language: Some("tr".into()),
        })
    }
}
impl SpeechOutput for Fixture {
    fn synthesize(&mut self, _: &str, _: &str, c: &TurnContext) -> Result<Wav, AgentError> {
        self.calls += 1;
        if self.cancel {
            c.cancellation().cancel();
        }
        if self.fail {
            Err(AgentError::Provider("fixture".into()))
        } else {
            Ok(wav())
        }
    }
}
fn driver() -> Fixture {
    Fixture {
        calls: 0,
        fail: false,
        invalid: false,
        cancel: false,
    }
}
#[test]
fn speech_events_are_ordered_and_never_contain_text_or_samples() {
    let mut d = driver();
    let mut events = vec![];
    let t = transcribe(&mut d, &wav(), Some("tr"), &ctx(), &mut events).unwrap();
    assert_eq!(t.text, "Türkçe özel metin");
    assert!(matches!(
        events.as_slice(),
        [
            AudioEvent::Started {
                operation: Operation::Transcribe
            },
            AudioEvent::InputValidated { .. },
            AudioEvent::Completed { .. }
        ]
    ));
    assert!(!serde_json::to_string(&events).unwrap().contains(&t.text));
    assert!(!format!("{t:?}").contains(&t.text));
    events.clear();
    let audio = synthesize(&mut d, "secret speech text", "voice_1", &ctx(), &mut events).unwrap();
    assert_eq!(audio.bytes(), wav().bytes());
    assert!(matches!(
        events.as_slice(),
        [
            AudioEvent::Started {
                operation: Operation::Synthesize
            },
            AudioEvent::Completed { audio: Some(_), .. }
        ]
    ));
    assert!(
        !serde_json::to_string(&events)
            .unwrap()
            .contains("secret speech text")
    );
}
#[test]
fn invalid_inputs_and_cancelled_context_do_not_dispatch_driver() {
    let mut d = driver();
    let mut events = vec![];
    for text in [
        "",
        " ",
        "x\0y",
        &"x".repeat(MAX_TEXT_BYTES + 1),
        &"🌍".repeat(4001),
    ] {
        assert!(synthesize(&mut d, text, "alloy", &ctx(), &mut events).is_err());
    }
    for voice in ["", "a\r\nb", "a/b", "a b"] {
        assert!(synthesize(&mut d, "text", voice, &ctx(), &mut events).is_err());
    }
    assert!(transcribe(&mut d, &wav(), Some("tr\r\n"), &ctx(), &mut events).is_err());
    let c = ctx();
    c.cancellation().cancel();
    assert_eq!(
        synthesize(&mut d, "text", "voice", &c, &mut events).unwrap_err(),
        AgentError::Cancelled
    );
    assert_eq!(d.calls, 0);
    assert!(events.is_empty());
}
#[test]
fn driver_failure_invalid_result_and_post_dispatch_cancellation_emit_failure() {
    for (fail, invalid, cancel) in [
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        let mut d = Fixture {
            fail,
            invalid,
            cancel,
            ..driver()
        };
        let mut events = vec![];
        assert!(transcribe(&mut d, &wav(), None, &ctx(), &mut events).is_err());
        assert_eq!(d.calls, 1);
        assert!(matches!(
            events.last(),
            Some(AudioEvent::Failed {
                operation: Operation::Transcribe
            })
        ));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AudioEvent::Completed { .. }))
        );
    }
}
#[test]
fn disconnected_observer_cannot_start_speech_effect() {
    struct Closed;
    impl AudioEvents for Closed {
        fn emit(&mut self, _: AudioEvent) -> Result<(), AgentError> {
            Err(AgentError::Cancelled)
        }
    }
    let mut d = driver();
    assert_eq!(
        synthesize(&mut d, "text", "voice", &ctx(), &mut Closed).unwrap_err(),
        AgentError::Cancelled
    );
    assert_eq!(d.calls, 0);
}
#[test]
fn workspace_publish_is_private_atomic_no_replace_and_roundtrips() {
    let root = tempfile::tempdir().unwrap();
    let w = AudioWorkspace::new(root.path()).unwrap();
    w.publish("out.wav", &wav(), &ctx()).unwrap();
    assert_eq!(w.read("out.wav", &ctx()).unwrap().bytes(), wav().bytes());
    let original = fs::read(root.path().join("out.wav")).unwrap();
    assert!(w.publish("out.wav", &wav(), &ctx()).is_err());
    assert_eq!(original, fs::read(root.path().join("out.wav")).unwrap());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(root.path().join("out.wav"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
#[test]
fn output_check_race_and_cancel_never_replace_existing_or_leave_partial_file() {
    let root = tempfile::tempdir().unwrap();
    let w = AudioWorkspace::new(root.path()).unwrap();
    w.check_destination("race.wav").unwrap();
    fs::write(root.path().join("race.wav"), "operator").unwrap();
    assert!(w.publish("race.wav", &wav(), &ctx()).is_err());
    assert_eq!(fs::read(root.path().join("race.wav")).unwrap(), b"operator");
    let c = ctx();
    c.cancellation().cancel();
    assert_eq!(
        w.publish("cancel.wav", &wav(), &c).unwrap_err(),
        AgentError::Cancelled
    );
    assert!(!root.path().join("cancel.wav").exists());
}
#[test]
fn workspace_denies_traversal_external_symlink_and_nonregular_audio() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("x.wav"), wav().bytes()).unwrap();
    let w = AudioWorkspace::new(root.path()).unwrap();
    for p in ["../x.wav", "/tmp/x.wav", "", "x\0.wav"] {
        assert!(w.read(p, &ctx()).is_err());
        assert!(w.publish(p, &wav(), &ctx()).is_err());
    }
    assert!(w.read(".", &ctx()).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink(outside.path(), root.path().join("escape")).unwrap();
        assert!(w.read("escape/x.wav", &ctx()).is_err());
        assert!(w.publish("escape/new.wav", &wav(), &ctx()).is_err());
        symlink(outside.path().join("x.wav"), root.path().join("link.wav")).unwrap();
        assert!(w.publish("link.wav", &wav(), &ctx()).is_err());
        assert!(
            std::process::Command::new("mkfifo")
                .arg(root.path().join("fifo"))
                .status()
                .unwrap()
                .success()
        );
        assert!(w.read("fifo", &ctx()).is_err());
    }
}
