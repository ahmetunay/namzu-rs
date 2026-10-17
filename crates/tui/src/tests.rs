use super::*;
use namzu_core::{Stats, ToolResult};

#[test]
fn failed_provisional_text_is_removed_and_uncertain_session_requires_explicit_new() {
    let mut app = model::App::new(None);
    app.append_input("request");
    assert_eq!(app.submit().as_deref(), Some("request"));
    app.event(Event::AssistantDelta {
        text: "not an accepted answer".into(),
    });
    app.event(Event::TurnFailed {
        error: "truncated stream".into(),
    });
    app.finish(Outcome {
        session_id: Some("failed-session".into()),
        error: Some("truncated stream".into()),
    });
    assert!(app.draft.is_empty() && app.blocked);
    assert!(
        !app.entries
            .iter()
            .any(|e| e.text.contains("not an accepted answer"))
    );
    app.append_input("do not auto replay");
    assert!(app.submit().is_none());
    assert_eq!(app.session_id.as_deref(), Some("failed-session"));
    app.new_conversation();
    assert!(app.session_id.is_none() && !app.blocked);
    app.append_input("fresh");
    assert_eq!(app.submit().as_deref(), Some("fresh"));
}

#[test]
fn unicode_input_and_transcript_are_bounded_and_control_sequences_are_inert() {
    let mut app = model::App::new(None);
    app.append_input("e\u{301}界");
    app.backspace();
    assert_eq!(app.input, "e\u{301}");
    app.backspace();
    assert!(app.input.is_empty());
    app.append_input(&"界".repeat(20000));
    assert!(app.input.len() <= model::INPUT_LIMIT && app.input.is_char_boundary(app.input.len()));
    app.input.clear();
    app.append_input("a\x1b[2J\r\nb\u{202e}");
    assert!(!app.input.contains(['\x1b', '\r', '\n', '\u{202e}']));
    for _ in 0..300 {
        app.event(Event::AssistantMessage {
            text: "界".repeat(30000),
            has_tool_calls: false,
        });
    }
    assert!(app.entries.len() <= 256);
    assert!(app.entries.iter().map(|e| e.text.len()).sum::<usize>() <= 256 * 1024);
    app.event(Event::AssistantDelta {
        text: "x".repeat(100000),
    });
    app.event(Event::AssistantDelta {
        text: "y".repeat(100000),
    });
    assert_eq!(app.draft.len(), 64 * 1024);
}

#[test]
fn accepted_text_replaces_draft_and_tool_preview_cannot_inject_terminal_controls() {
    let mut app = model::App::new(None);
    app.append_input("hi");
    app.submit();
    app.event(Event::AssistantDelta { text: "old".into() });
    app.event(Event::AssistantMessage {
        text: "accepted".into(),
        has_tool_calls: true,
    });
    app.event(Event::ToolStarted {
        id: "x".into(),
        name: "read_file".into(),
    });
    app.event(Event::ToolFinished {
        id: "x".into(),
        name: "read_file".into(),
        result: ToolResult::ok("\x1b]52;c;secret\x07"),
    });
    app.event(Event::TurnCompleted {
        stats: Stats {
            provider_calls: 2,
            tool_calls: 1,
        },
    });
    app.finish(Outcome {
        session_id: Some("completed".into()),
        error: None,
    });
    assert!(app.draft.is_empty() && !app.busy && !app.blocked);
    assert_eq!(
        app.entries.iter().filter(|e| e.text == "accepted").count(),
        1
    );
    assert!(
        app.entries
            .iter()
            .all(|e| !e.text.contains(['\x1b', '\x07']))
    );
    assert_eq!(app.session_id.as_deref(), Some("completed"));
}

#[test]
fn dropping_active_turn_cancels_and_unblocks_a_full_event_channel_then_joins() {
    let signal = Cancellation::default();
    let child_signal = signal.clone();
    let (sender, receiver) = mpsc::sync_channel(1);
    let (completed, observed) = mpsc::channel();
    sender.send(Event::TurnStarted).unwrap();
    let worker = thread::spawn(move || {
        assert_eq!(
            ChannelEvents(sender).emit(Event::ProviderStarted { step: 1 }),
            Err(AgentError::Cancelled)
        );
        assert!(child_signal.is_cancelled());
        completed.send(()).unwrap();
        Outcome {
            session_id: None,
            error: Some("cancelled".into()),
        }
    });
    let active = ActiveTurn {
        receiver: Some(receiver),
        worker: Some(worker),
        cancellation: signal.clone(),
    };
    drop(active);
    observed.recv().unwrap();
    assert!(signal.is_cancelled());
}
