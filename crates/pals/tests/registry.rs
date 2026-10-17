use namzu_core::{
    AgentError, Cancellation, Completion, Event, Limits, Message, NoopObserver, Observer, Provider,
    ScriptedProvider, Stats, ToolCall, ToolDefinition, ToolResult, Tools, TurnContext,
};
use namzu_pals::{PalCreate, PalDefinition, PalStatus, PalUpdate, Registry, TaskStatus};
use namzu_session::{PrivateStore, SessionStatus};
use namzu_tools::{TerminalTools, ToolPolicy};
use serde_json::json;
use std::{path::Path, time::Duration};

fn registry(root: &Path) -> Registry {
    Registry::open(root.join("state"), root.join("workspaces")).unwrap()
}
fn create(reg: &Registry) -> PalDefinition {
    reg.create(PalCreate {
        owner: "owner".into(),
        name: "Ada".into(),
        purpose: "review".into(),
        route: "fixture-model-v1".into(),
        tools: None,
    })
    .unwrap()
}
fn context() -> TurnContext {
    TurnContext::new(Duration::from_secs(30), Cancellation::default()).unwrap()
}
fn text() -> ScriptedProvider {
    ScriptedProvider(
        vec![Completion {
            text: "done".into(),
            tool_calls: vec![],
        }]
        .into(),
    )
}
fn tools(pal: &PalDefinition) -> TerminalTools {
    TerminalTools::new(
        &pal.workspace,
        ToolPolicy {
            allow_write: true,
            ..ToolPolicy::default()
        },
    )
    .unwrap()
}
fn writes() -> ScriptedProvider {
    ScriptedProvider(
        vec![
            Completion {
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "write-1".into(),
                    name: "write_file".into(),
                    arguments: json!({"path":"answer.txt","content":"once"}),
                }],
            },
            Completion {
                text: "done".into(),
                tool_calls: vec![],
            },
        ]
        .into(),
    )
}
fn allow_write(reg: &Registry, pal: &PalDefinition) -> PalDefinition {
    reg.update(
        "owner",
        &pal.id,
        pal.revision,
        PalUpdate {
            tools: Some(["write_file".into(), "read_file".into()].into()),
            ..PalUpdate::default()
        },
    )
    .unwrap()
}

#[test]
fn identities_private_workspaces_and_immutable_revisions_survive_reopen() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let pal = create(&reg);
    let other = create(&reg);
    assert_ne!(pal.id, other.id);
    assert_ne!(pal.workspace, other.workspace);
    let updated = reg
        .update(
            "owner",
            &pal.id,
            1,
            PalUpdate {
                name: Some("New name".into()),
                route: Some("fixture-model-v2".into()),
                ..PalUpdate::default()
            },
        )
        .unwrap();
    assert_eq!(updated.revision, 2);
    assert_eq!(updated.workspace, pal.workspace);
    assert!(
        reg.update("owner", &pal.id, 1, PalUpdate::default())
            .is_err()
    );
    drop(reg);
    let reg = registry(root.path());
    assert_eq!(reg.get("owner", &pal.id).unwrap().name, "New name");
    assert_eq!(
        reg.revision("owner", &pal.id, 1).unwrap().route,
        "fixture-model-v1"
    );
    assert_eq!(reg.list("owner").unwrap().len(), 2);
    assert!(reg.list("intruder").unwrap().is_empty());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&pal.workspace)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(root.path().join("state").join(format!("{}.json", pal.id)))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn owner_mismatch_blocks_reads_changes_and_task_admission() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let pal = create(&reg);
    assert!(reg.get("intruder", &pal.id).is_err());
    assert!(reg.tasks("intruder", &pal.id).is_err());
    assert!(
        reg.update("intruder", &pal.id, 1, PalUpdate::default())
            .is_err()
    );
    assert!(reg.queue_task("intruder", &pal.id, "task", "x").is_err());
    reg.queue_task("owner", &pal.id, "task", "x").unwrap();
    assert!(reg.start_task("intruder", &pal.id, "task").is_err());
    assert!(reg.session("intruder", &pal.id, "task").is_err());
    assert!(
        reg.acknowledge_interruption("intruder", &pal.id, "task")
            .is_err()
    );
}

#[test]
fn queued_task_pins_profile_and_complete_session_can_be_explicitly_followed() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let pal = create(&reg);
    let first = reg
        .queue_task("owner", &pal.id, "first", "first question")
        .unwrap();
    reg.update(
        "owner",
        &pal.id,
        1,
        PalUpdate {
            route: Some("new-route".into()),
            ..PalUpdate::default()
        },
    )
    .unwrap();
    let lease = reg.start_task("owner", &pal.id, "first").unwrap();
    assert_eq!(lease.definition().route, "fixture-model-v1");
    lease
        .execute(
            &mut text(),
            &mut tools(&pal),
            &context(),
            Limits::default(),
            &mut NoopObserver,
        )
        .unwrap();
    let completed = reg.session("owner", &pal.id, "first").unwrap();
    assert_eq!(completed.status, SessionStatus::Complete);
    let follow = reg
        .queue_followup("owner", &pal.id, "second", "first", "followup")
        .unwrap();
    assert_eq!(follow.revision, 1);
    assert_eq!(follow.follows.as_deref(), Some("first"));
    assert_ne!(first.session_id, follow.session_id);
    struct Inspect;
    impl Provider for Inspect {
        fn complete(
            &mut self,
            messages: &[Message],
            _: &[ToolDefinition],
            _: &TurnContext,
        ) -> Result<Completion, AgentError> {
            assert_eq!(messages.len(), 3);
            assert!(matches!(&messages[2],Message::User{content} if content=="followup"));
            Ok(Completion {
                text: "followed".into(),
                tool_calls: vec![],
            })
        }
    }
    reg.start_task("owner", &pal.id, "second")
        .unwrap()
        .execute(
            &mut Inspect,
            &mut tools(&pal),
            &context(),
            Limits::default(),
            &mut NoopObserver,
        )
        .unwrap();
    assert_eq!(
        reg.session("owner", &pal.id, "first").unwrap().messages,
        completed.messages
    );
    let fresh = reg.queue_task("owner", &pal.id, "fresh", "new").unwrap();
    assert_eq!(fresh.revision, 2);
    assert!(reg.start_task("owner", &pal.id, "first").is_err());
    assert!(
        reg.queue_task("owner", &pal.id, "first", "duplicate")
            .is_err()
    );
}

#[test]
fn default_profile_cannot_gain_write_from_host_tools() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let pal = create(&reg);
    reg.queue_task("owner", &pal.id, "t", "work").unwrap();
    let turn = reg
        .start_task("owner", &pal.id, "t")
        .unwrap()
        .execute(
            &mut writes(),
            &mut tools(&pal),
            &context(),
            Limits::default(),
            &mut NoopObserver,
        )
        .unwrap();
    assert!(
        turn.messages
            .iter()
            .any(|m| matches!(m, Message::Tool { is_error: true, .. }))
    );
    assert!(!pal.workspace.join("answer.txt").exists());
}

#[test]
fn durable_pending_precedes_effect_and_complete_clears_it() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let pal = allow_write(&reg, &create(&reg));
    reg.queue_task("owner", &pal.id, "t", "write").unwrap();
    struct Audit<'a> {
        inner: TerminalTools,
        reg: &'a Registry,
        id: String,
        seen: bool,
    }
    impl Tools for Audit<'_> {
        fn definitions(&self) -> Vec<ToolDefinition> {
            self.inner.definitions()
        }
        fn call(&mut self, call: &ToolCall, ctx: &TurnContext) -> Result<ToolResult, AgentError> {
            assert_eq!(
                self.reg
                    .session("owner", &self.id, "t")
                    .unwrap()
                    .pending_call
                    .as_deref(),
                Some(call.id.as_str())
            );
            self.seen = true;
            self.inner.call(call, ctx)
        }
    }
    let mut audit = Audit {
        inner: tools(&pal),
        reg: &reg,
        id: pal.id.clone(),
        seen: false,
    };
    reg.start_task("owner", &pal.id, "t")
        .unwrap()
        .execute(
            &mut writes(),
            &mut audit,
            &context(),
            Limits::default(),
            &mut NoopObserver,
        )
        .unwrap();
    assert!(audit.seen);
    assert_eq!(
        std::fs::read_to_string(pal.workspace.join("answer.txt")).unwrap(),
        "once"
    );
    let session = reg.session("owner", &pal.id, "t").unwrap();
    assert!(session.pending_call.is_none());
    assert_eq!(session.stats.tool_calls, 1);
    assert_eq!(
        reg.tasks("owner", &pal.id).unwrap()[0].status,
        TaskStatus::Complete
    );
}

#[test]
fn host_approval_denial_is_preserved() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let pal = allow_write(&reg, &create(&reg));
    reg.queue_task("owner", &pal.id, "t", "write").unwrap();
    struct Deny;
    impl Observer for Deny {
        fn approve(&mut self, _: &ToolCall, _: &TurnContext) -> Result<bool, AgentError> {
            Ok(false)
        }
        fn checkpoint(
            &mut self,
            _: &[Message],
            _: &Stats,
            _: Option<&ToolCall>,
        ) -> Result<(), AgentError> {
            Ok(())
        }
    }
    reg.start_task("owner", &pal.id, "t")
        .unwrap()
        .execute(
            &mut writes(),
            &mut tools(&pal),
            &context(),
            Limits::default(),
            &mut Deny,
        )
        .unwrap();
    assert!(!pal.workspace.join("answer.txt").exists());
}

#[test]
fn pause_and_archive_stop_admission_without_resetting_pinned_profiles() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let pal = create(&reg);
    reg.queue_task("owner", &pal.id, "t", "x").unwrap();
    reg.update(
        "owner",
        &pal.id,
        1,
        PalUpdate {
            status: Some(PalStatus::Paused),
            ..PalUpdate::default()
        },
    )
    .unwrap();
    assert!(reg.start_task("owner", &pal.id, "t").is_err());
    assert!(reg.queue_task("owner", &pal.id, "new", "x").is_err());
    reg.update(
        "owner",
        &pal.id,
        2,
        PalUpdate {
            status: Some(PalStatus::Active),
            ..PalUpdate::default()
        },
    )
    .unwrap();
    let lease = reg.start_task("owner", &pal.id, "t").unwrap();
    assert_eq!(lease.definition().revision, 1);
    drop(lease);
    reg.update(
        "owner",
        &pal.id,
        3,
        PalUpdate {
            status: Some(PalStatus::Archived),
            ..PalUpdate::default()
        },
    )
    .unwrap();
    assert!(
        reg.update(
            "owner",
            &pal.id,
            4,
            PalUpdate {
                status: Some(PalStatus::Active),
                ..PalUpdate::default()
            }
        )
        .is_err()
    );
    assert!(reg.queue_task("owner", &pal.id, "next", "x").is_err());
}

#[test]
fn pause_after_presentation_callback_stops_before_paid_provider_request() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let pal = create(&reg);
    reg.queue_task("owner", &pal.id, "t", "x").unwrap();
    struct Pause<'a> {
        reg: &'a Registry,
        id: String,
    }
    impl Observer for Pause<'_> {
        fn event(&mut self, event: Event) -> Result<(), AgentError> {
            if matches!(event, Event::ProviderStarted { .. }) {
                self.reg.update(
                    "owner",
                    &self.id,
                    1,
                    PalUpdate {
                        status: Some(PalStatus::Paused),
                        ..PalUpdate::default()
                    },
                )?;
            }
            Ok(())
        }
        fn checkpoint(
            &mut self,
            _: &[Message],
            _: &Stats,
            _: Option<&ToolCall>,
        ) -> Result<(), AgentError> {
            Ok(())
        }
    }
    let mut provider = text();
    let mut pause = Pause {
        reg: &reg,
        id: pal.id.clone(),
    };
    assert!(
        reg.start_task("owner", &pal.id, "t")
            .unwrap()
            .execute(
                &mut provider,
                &mut tools(&pal),
                &context(),
                Limits::default(),
                &mut pause
            )
            .is_err()
    );
    assert_eq!(provider.0.len(), 1);
    assert_eq!(
        reg.tasks("owner", &pal.id).unwrap()[0].status,
        TaskStatus::Failed
    );
}

#[test]
fn drop_and_cancel_are_terminal_and_never_replay_failed_history() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let pal = create(&reg);
    reg.queue_task("owner", &pal.id, "dropped", "x").unwrap();
    drop(reg.start_task("owner", &pal.id, "dropped").unwrap());
    assert_eq!(
        reg.tasks("owner", &pal.id).unwrap()[0].status,
        TaskStatus::Interrupted
    );
    assert!(reg.start_task("owner", &pal.id, "dropped").is_err());
    assert!(
        reg.queue_followup("owner", &pal.id, "bad", "dropped", "x")
            .is_err()
    );
    reg.queue_task("owner", &pal.id, "cancelled", "x").unwrap();
    let ctx = context();
    ctx.cancellation().cancel();
    assert_eq!(
        reg.start_task("owner", &pal.id, "cancelled")
            .unwrap()
            .execute(
                &mut text(),
                &mut tools(&pal),
                &ctx,
                Limits::default(),
                &mut NoopObserver
            )
            .unwrap_err(),
        AgentError::Cancelled
    );
    assert_eq!(
        reg.tasks("owner", &pal.id).unwrap()[1].status,
        TaskStatus::Cancelled
    );
}

#[test]
fn process_lock_prevents_two_tasks_from_owning_same_pal_workspace() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let pal = create(&reg);
    reg.queue_task("owner", &pal.id, "one", "x").unwrap();
    reg.queue_task("owner", &pal.id, "two", "x").unwrap();
    let lease = reg.start_task("owner", &pal.id, "one").unwrap();
    let other = registry(root.path());
    assert!(other.start_task("owner", &pal.id, "two").is_err());
    assert!(
        other
            .acknowledge_interruption("owner", &pal.id, "one")
            .is_err()
    );
    let output = fixture_process(root.path(), &pal.id, "lock");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    drop(lease);
    assert!(reg.start_task("owner", &pal.id, "two").is_ok());
}

fn fixture_process(root: &Path, id: &str, mode: &str) -> std::process::Output {
    std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "subprocess_fixture", "--nocapture"])
        .env("NAMZU_PALS_FIXTURE_ROOT", root)
        .env("NAMZU_PALS_FIXTURE_ID", id)
        .env("NAMZU_PALS_FIXTURE_MODE", mode)
        .output()
        .unwrap()
}
#[test]
fn subprocess_fixture() {
    let Some(root) = std::env::var_os("NAMZU_PALS_FIXTURE_ROOT") else {
        return;
    };
    let id = std::env::var("NAMZU_PALS_FIXTURE_ID").unwrap();
    let mode = std::env::var("NAMZU_PALS_FIXTURE_MODE").unwrap();
    let reg = registry(Path::new(&root));
    if mode == "lock" {
        assert!(reg.start_task("owner", &id, "two").is_err());
        return;
    }
    let pal = reg.get("owner", &id).unwrap();
    struct Crash(TerminalTools);
    impl Tools for Crash {
        fn definitions(&self) -> Vec<ToolDefinition> {
            self.0.definitions()
        }
        fn call(&mut self, call: &ToolCall, ctx: &TurnContext) -> Result<ToolResult, AgentError> {
            let result = self.0.call(call, ctx)?;
            assert!(!result.is_error);
            // Exit after the effect, without Rust Drop or a post-effect checkpoint.
            std::process::exit(0)
        }
    }
    let _ = reg.start_task("owner", &id, "crash").unwrap().execute(
        &mut writes(),
        &mut Crash(tools(&pal)),
        &context(),
        Limits::default(),
        &mut NoopObserver,
    );
    panic!("fixture must exit from inside tool effect")
}

#[test]
fn real_process_crash_preserves_uncertain_effect_and_requires_explicit_ack() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let pal = allow_write(&reg, &create(&reg));
    reg.queue_task("owner", &pal.id, "crash", "write once")
        .unwrap();
    let output = fixture_process(root.path(), &pal.id, "crash");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(pal.workspace.join("answer.txt")).unwrap(),
        "once"
    );
    let reg = registry(root.path());
    assert_eq!(
        reg.tasks("owner", &pal.id).unwrap()[0].status,
        TaskStatus::Running
    );
    assert_eq!(
        reg.session("owner", &pal.id, "crash")
            .unwrap()
            .pending_call
            .as_deref(),
        Some("write-1")
    );
    assert!(reg.start_task("owner", &pal.id, "crash").is_err());
    assert!(
        reg.queue_followup("owner", &pal.id, "retry", "crash", "x")
            .is_err()
    );
    reg.queue_task("owner", &pal.id, "next", "new").unwrap();
    assert!(reg.start_task("owner", &pal.id, "next").is_err());
    reg.acknowledge_interruption("owner", &pal.id, "crash")
        .unwrap();
    assert_eq!(
        reg.tasks("owner", &pal.id).unwrap()[0].status,
        TaskStatus::Interrupted
    );
    assert!(reg.start_task("owner", &pal.id, "crash").is_err());
    reg.start_task("owner", &pal.id, "next")
        .unwrap()
        .execute(
            &mut text(),
            &mut tools(&pal),
            &context(),
            Limits::default(),
            &mut NoopObserver,
        )
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(pal.workspace.join("answer.txt")).unwrap(),
        "once"
    );
}

#[test]
fn missing_or_symlinked_workspace_is_never_recreated_or_accepted() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let pal = create(&reg);
    std::fs::remove_dir(&pal.workspace).unwrap();
    assert!(reg.get("owner", &pal.id).is_err());
    assert!(!pal.workspace.exists());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.path().join("state"), &pal.workspace).unwrap();
        assert!(reg.get("owner", &pal.id).is_err());
    }
}

#[test]
fn malformed_record_and_session_route_are_rejected_without_running_provider() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let pal = create(&reg);
    reg.queue_task("owner", &pal.id, "t", "x").unwrap();
    let state = PrivateStore::open(root.path().join("state")).unwrap();
    let private = state.open_child(&pal.id).unwrap();
    let mut session = reg.session("owner", &pal.id, "t").unwrap();
    session.route = "wrong-route".into();
    session.save(&private).unwrap();
    assert!(reg.start_task("owner", &pal.id, "t").is_err());
    let name = format!("{}.json", pal.id);
    let mut record: serde_json::Value = state.read(&name).unwrap().unwrap();
    record["revisions"][0]["workspace"] = json!(root.path());
    state.write(&name, &record).unwrap();
    assert!(reg.get("owner", &pal.id).is_err());
}

#[test]
fn roots_must_be_separate_and_record_limits_do_not_overwrite() {
    let root = tempfile::tempdir().unwrap();
    assert!(Registry::open(root.path(), root.path()).is_err());
    assert!(Registry::open(root.path().join("nested"), root.path()).is_err());
    let reg = registry(root.path());
    let pal = create(&reg);
    assert!(reg.queue_task("owner", &pal.id, "../bad", "x").is_err());
    assert!(
        reg.queue_task("owner", &pal.id, "t", &"x".repeat(16 * 1024 + 1))
            .is_err()
    );
    assert_eq!(reg.get("owner", &pal.id).unwrap().revision, 1);
}
