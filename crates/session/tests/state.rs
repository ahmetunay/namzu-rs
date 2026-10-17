use namzu_core::*;
use namzu_session::*;
use serde_json::json;

#[test]
fn private_atomic_roundtrip_and_lock_contention() {
    let root = tempfile::tempdir().unwrap();
    let store = PrivateStore::open(root.path()).unwrap();
    store.write("record.json", &json!({"value":1})).unwrap();
    store.write("record.json", &json!({"value":2})).unwrap();
    assert_eq!(
        store
            .read::<serde_json::Value>("record.json")
            .unwrap()
            .unwrap()["value"],
        2
    );
    let lock = store.lock("record").unwrap();
    assert!(store.lock("record").is_err());
    drop(lock);
    assert!(store.lock("record").is_ok());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(root.path().join("record.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn traversal_and_corrupt_state_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let store = PrivateStore::open(root.path()).unwrap();
    assert!(store.write("../outside", &json!({})).is_err());
    assert!(store.lock("../outside").is_err());
    std::fs::write(root.path().join("bad.json"), b"broken").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            root.path().join("bad.json"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    let error = store
        .read::<serde_json::Value>("bad.json")
        .unwrap_err()
        .to_string();
    assert!(!error.contains("broken"));
}

#[cfg(unix)]
#[test]
fn shared_root_symlink_and_hardlinked_state_are_refused() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = tempfile::tempdir().unwrap();
    let store = PrivateStore::open(root.path()).unwrap();
    store
        .write("original.json", &json!({"secret":"fixture"}))
        .unwrap();
    symlink("original.json", root.path().join("link.json")).unwrap();
    assert!(store.read::<serde_json::Value>("link.json").is_err());
    std::fs::hard_link(
        root.path().join("original.json"),
        root.path().join("hard.json"),
    )
    .unwrap();
    assert!(store.read::<serde_json::Value>("hard.json").is_err());
    let parent = tempfile::tempdir().unwrap();
    symlink(root.path(), parent.path().join("link")).unwrap();
    assert!(PrivateStore::open(parent.path().join("link")).is_err());
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(PrivateStore::open(root.path()).is_err());
}

#[test]
fn resume_requires_completed_session_workspace_and_route() {
    let root = tempfile::tempdir().unwrap();
    let store = PrivateStore::open(root.path()).unwrap();
    let cwd = root.path().canonicalize().unwrap();
    let mut session = Session::new(new_id(), cwd.clone(), "route".into()).unwrap();
    session.save(&store).unwrap();
    let loaded = Session::load(&store, &session.id).unwrap();
    assert!(loaded.require_resumable(&cwd, "route").is_ok());
    assert!(loaded.require_resumable(&cwd, "other").is_err());
    assert!(
        loaded
            .require_resumable(std::path::Path::new("/other"), "route")
            .is_err()
    );
    session.status = SessionStatus::Running;
    assert!(session.require_resumable(&cwd, "route").is_err());
    session.status = SessionStatus::Complete;
    session.pending_call = Some("effect".into());
    assert!(session.require_resumable(&cwd, "route").is_err());
}

#[test]
fn uncertain_effect_is_durable_and_cannot_be_replayed() {
    struct FailingEffect;
    impl Tools for FailingEffect {
        fn definitions(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                name: "effect".into(),
                description: "fixture".into(),
                parameters: json!({"type":"object"}),
            }]
        }
        fn call(&mut self, _: &ToolCall, _: &TurnContext) -> Result<ToolResult, AgentError> {
            Err(AgentError::Timeout)
        }
    }
    let root = tempfile::tempdir().unwrap();
    let store = PrivateStore::open(root.path()).unwrap();
    let cwd = root.path().canonicalize().unwrap();
    let mut session = Session::new(new_id(), cwd.clone(), "fixture".into()).unwrap();
    let id = session.id.clone();
    let mut provider = ScriptedProvider(
        vec![Completion {
            text: "".into(),
            tool_calls: vec![ToolCall {
                id: "effect-id".into(),
                name: "effect".into(),
                arguments: json!({}),
            }],
        }]
        .into(),
    );
    let result = run_history(
        &mut provider,
        &mut FailingEffect,
        vec![Message::User {
            content: "test".into(),
        }],
        Limits::default(),
        Cancellation::default(),
        &mut Journal {
            store: &store,
            session: &mut session,
        },
    );
    assert_eq!(result.unwrap_err(), AgentError::Timeout);
    let loaded = Session::load(&store, &id).unwrap();
    assert_eq!(loaded.pending_call.as_deref(), Some("effect-id"));
    assert_eq!(loaded.status, SessionStatus::Running);
    assert!(loaded.require_resumable(&cwd, "fixture").is_err());
}

#[test]
fn failed_checkpoint_prevents_effect() {
    struct Blocker;
    impl Observer for Blocker {
        fn checkpoint(
            &mut self,
            _: &[Message],
            _: &Stats,
            pending: Option<&ToolCall>,
        ) -> Result<(), AgentError> {
            if pending.is_some() {
                Err(AgentError::State("fixture disk failure"))
            } else {
                Ok(())
            }
        }
    }
    struct MustNotRun;
    impl Tools for MustNotRun {
        fn definitions(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                name: "effect".into(),
                description: "fixture".into(),
                parameters: json!({"type":"object"}),
            }]
        }
        fn call(&mut self, _: &ToolCall, _: &TurnContext) -> Result<ToolResult, AgentError> {
            panic!("effect ran before durable admission")
        }
    }
    let mut provider = ScriptedProvider(
        vec![Completion {
            text: "".into(),
            tool_calls: vec![ToolCall {
                id: "effect-id".into(),
                name: "effect".into(),
                arguments: json!({}),
            }],
        }]
        .into(),
    );
    assert!(matches!(
        run_history(
            &mut provider,
            &mut MustNotRun,
            vec![Message::User {
                content: "test".into()
            }],
            Limits::default(),
            Cancellation::default(),
            &mut Blocker
        ),
        Err(AgentError::State(_))
    ));
}

#[test]
fn unresolved_and_reused_historical_tool_calls_are_rejected() {
    struct NoneTools;
    impl Tools for NoneTools {
        fn definitions(&self) -> Vec<ToolDefinition> {
            vec![]
        }
        fn call(&mut self, _: &ToolCall, _: &TurnContext) -> Result<ToolResult, AgentError> {
            panic!()
        }
    }
    let call = ToolCall {
        id: "old".into(),
        name: "none".into(),
        arguments: json!({}),
    };
    let history = vec![Message::Assistant {
        content: "".into(),
        tool_calls: vec![call.clone()],
    }];
    let mut provider = ScriptedProvider(vec![Completion::default()].into());
    assert!(
        run_history(
            &mut provider,
            &mut NoneTools,
            history.clone(),
            Limits::default(),
            Cancellation::default(),
            &mut NoopObserver
        )
        .is_err()
    );
    assert_eq!(provider.0.len(), 1);
    let mut history = history;
    history.push(Message::Tool {
        tool_call_id: "old".into(),
        content: "done".into(),
        is_error: false,
    });
    history.push(Message::User {
        content: "next".into(),
    });
    let mut provider = ScriptedProvider(
        vec![Completion {
            text: "".into(),
            tool_calls: vec![call],
        }]
        .into(),
    );
    assert!(
        run_history(
            &mut provider,
            &mut NoneTools,
            history,
            Limits::default(),
            Cancellation::default(),
            &mut NoopObserver
        )
        .is_err()
    );
}
