use super::*;
use namzu_core::Cancellation;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
    thread,
    time::Duration,
};
fn ctx() -> TurnContext {
    TurnContext::new(Duration::from_secs(10), Cancellation::default()).unwrap()
}
fn wav() -> Wav {
    Wav::pcm16(16000, 1, &vec![42; 160]).unwrap()
}
fn request(stream: &mut std::net::TcpStream) -> (String, Vec<u8>) {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        header.push(byte[0]);
        assert!(header.len() < 16384);
    }
    let header = String::from_utf8(header).unwrap();
    let length: usize = header
        .lines()
        .find_map(|l| {
            l.to_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse().unwrap())
        })
        .unwrap();
    assert!(length < MAX_AUDIO_BYTES + 4096);
    let mut body = vec![0; length];
    stream.read_exact(&mut body).unwrap();
    (header, body)
}
fn serve(
    status: u16,
    extra: &str,
    payload: Vec<u8>,
    check: impl FnOnce(&str, &[u8]) + Send + 'static,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let extra = extra.to_string();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let (header, body) = request(&mut stream);
        check(&header, &body);
        stream.write_all(format!("HTTP/1.1 {status} Fixture\r\nConnection: close\r\nContent-Length: {}\r\n{extra}\r\n",payload.len()).as_bytes()).unwrap();
        let _ = stream.write_all(&payload);
    });
    (base, handle)
}
fn client(base: &str) -> SpeechClient {
    let mut c =
        SpeechClient::new(base, "fixture-key-not-real", "stt-fixture", "tts-fixture").unwrap();
    c.client = Client::builder()
        .redirect(Policy::none())
        .no_proxy()
        .build()
        .unwrap();
    c
}
#[test]
fn multipart_transcription_sends_exact_wav_and_fixed_private_filename() {
    let expected = wav().bytes().to_vec();
    let (base, server) = serve(
        200,
        "Content-Type: application/json\r\n",
        r#"{"text":"Merhaba Türkçe ✓","language":"tr"}"#.as_bytes().to_vec(),
        move |header, body| {
            assert!(header.starts_with("POST /v1/audio/transcriptions HTTP/1.1"));
            assert!(
                header
                    .to_lowercase()
                    .contains("authorization: bearer fixture-key-not-real")
            );
            let boundary = header
                .lines()
                .find(|l| l.to_lowercase().starts_with("content-type:"))
                .unwrap()
                .split("boundary=")
                .nth(1)
                .unwrap();
            let text = String::from_utf8_lossy(body);
            assert!(text.contains("name=\"model\"\r\n\r\nstt-fixture\r\n"));
            assert!(text.contains("name=\"language\"\r\n\r\ntr\r\n"));
            assert!(text.contains("filename=\"input.wav\"\r\nContent-Type: audio/wav\r\n\r\n"));
            assert!(body.windows(expected.len()).any(|b| b == expected));
            assert!(body.ends_with(format!("\r\n--{boundary}--\r\n").as_bytes()));
        },
    );
    let t = client(&base)
        .transcribe(&wav(), Some("tr"), &ctx())
        .unwrap();
    assert_eq!(t.text, "Merhaba Türkçe ✓");
    server.join().unwrap();
}
#[test]
fn speech_json_and_native_pcm16_response_roundtrip() {
    let audio = wav().bytes().to_vec();
    let expected = audio.clone();
    let (base, server) = serve(200, "Content-Type: audio/wav\r\n", audio, |header, body| {
        assert!(header.starts_with("POST /v1/audio/speech HTTP/1.1"));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(body).unwrap(),
            json!({"model":"tts-fixture","input":"Türkçe ✓","voice":"alloy","response_format":"wav"})
        );
    });
    let audio = client(&base)
        .synthesize("Türkçe ✓", "alloy", &ctx())
        .unwrap();
    assert_eq!(audio.bytes(), expected);
    server.join().unwrap();
}
#[test]
fn credential_url_and_header_validation_do_not_disclose_secrets() {
    for url in [
        "http://example.com/v1",
        "https://u:private@example.com/v1",
        "https://example.com/v1?q=private",
        "https://example.com/v1#x",
        "file:///tmp/wav",
    ] {
        assert!(SpeechClient::new(url, "secret", "a", "b").is_err());
    }
    for key in ["", "secret\r\ninjected"] {
        let e = SpeechClient::new("https://example.com/v1", key, "a", "b")
            .err()
            .unwrap();
        assert!(!e.to_string().contains("secret"));
    }
    assert!(SpeechClient::new("https://example.com/v1", "key", "bad\r\nmodel", "ok").is_err());
}
#[test]
fn error_bodies_and_redirects_are_not_replayed_or_logged() {
    for status in [302, 429, 503] {
        let (base, server) = serve(
            status,
            "Location: http://127.0.0.1:1/forbidden\r\n",
            b"private-key-and-private-text".to_vec(),
            |_, _| (),
        );
        let e = client(&base)
            .synthesize("private text", "alloy", &ctx())
            .unwrap_err();
        assert!(e.to_string().contains(&status.to_string()));
        assert!(!e.to_string().contains("private"));
        server.join().unwrap();
    }
}
#[test]
fn malformed_and_oversized_speech_responses_are_rejected() {
    for body in [b"not wav".to_vec(), vec![0; MAX_AUDIO_BYTES + 1]] {
        let (base, server) = serve(200, "", body, |_, _| ());
        assert!(client(&base).synthesize("test", "alloy", &ctx()).is_err());
        server.join().unwrap();
    }
    for body in [
        br#"{"text":false}"#.to_vec(),
        json!({"text":"x".repeat(namzu_audio::MAX_TRANSCRIPT_BYTES+1)})
            .to_string()
            .into_bytes(),
        vec![b'x'; 128 * 1024 + 1],
    ] {
        let (base, server) = serve(200, "", body, |_, _| ());
        assert!(client(&base).transcribe(&wav(), None, &ctx()).is_err());
        server.join().unwrap();
    }
}
#[test]
fn cancel_after_response_headers_drops_connection_and_never_finishes_audio() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let (start, started) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        request(&mut stream);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100000\r\n\r\nR")
            .unwrap();
        start.send(()).unwrap();
        let mut byte = [0];
        assert_eq!(stream.read(&mut byte).unwrap(), 0);
    });
    let c = ctx();
    let cancel = c.cancellation();
    let worker = thread::spawn(move || {
        started.recv().unwrap();
        cancel.cancel();
    });
    let e = client(&base).synthesize("test", "alloy", &c).unwrap_err();
    assert_eq!(e, AgentError::Cancelled);
    worker.join().unwrap();
    server.join().unwrap();
}
#[test]
fn stalled_headers_observe_deadline_without_background_request() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        request(&mut stream);
        let mut byte = [0];
        assert_eq!(stream.read(&mut byte).unwrap(), 0);
    });
    let c = TurnContext::new(Duration::from_millis(300), Cancellation::default()).unwrap();
    assert_eq!(
        client(&base).synthesize("test", "alloy", &c).unwrap_err(),
        AgentError::Timeout
    );
    server.join().unwrap();
}

#[test]
fn streamed_unknown_wav_sizes_normalize_after_complete_body() {
    let expected = wav().bytes().to_vec();
    let mut streamed = expected.clone();
    streamed[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    streamed[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Wav::parse(streamed.clone()).is_err());
    let (base, server) = serve(200, "Content-Type: audio/wav\r\n", streamed, |_, _| ());
    let audio = client(&base).synthesize("stream", "alloy", &ctx()).unwrap();
    assert_eq!(audio.bytes(), expected);
    server.join().unwrap();
    let mut invalid = expected.clone();
    invalid[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    invalid[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Wav::from_http_body(invalid).is_err());
}
