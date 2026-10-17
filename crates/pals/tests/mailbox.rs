use namzu_core::{
    AgentError, Cancellation, Completion, Event, Limits, Message, NoopObserver, Observer, Provider,
    ScriptedProvider, Stats, ToolCall, ToolDefinition, ToolResult, Tools, TurnContext,
};
use namzu_pals::{
    BatchBudget, DeliveryState, MailMessage, MailSender, Mailbox, MailboxClock, MailboxHost,
    MailboxRuntime, PalCreate, PalDefinition, PalStatus, PalUpdate, Registry, TaskStatus,
};
use namzu_session::{PrivateStore, SessionStatus};
use namzu_tools::{TerminalTools, ToolPolicy};
use serde_json::json;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

#[derive(Clone)]
struct Clock(Arc<AtomicU64>);
impl Clock {
    fn new() -> Self {
        Self(Arc::new(AtomicU64::new(1000)))
    }
    fn set(&self, t: u64) {
        self.0.store(t, Ordering::SeqCst);
    }
}
impl MailboxClock for Clock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}
fn registry(root: &Path) -> Registry {
    Registry::open(root.join("state"), root.join("workspaces")).unwrap()
}
fn pal(reg: &Registry, write: bool) -> PalDefinition {
    reg.create(PalCreate {
        owner: "owner".into(),
        name: "Reader".into(),
        purpose: "fixture".into(),
        route: "fixture-v1".into(),
        tools: write.then(|| ["write_file".into(), "read_file".into()].into()),
    })
    .unwrap()
}
fn inbox<'a>(reg: &'a Registry, pal: &PalDefinition, clock: &Clock) -> Mailbox<'a, Clock> {
    reg.mailbox("owner", &pal.id)
        .unwrap()
        .with_clock(clock.clone())
}
fn context() -> TurnContext {
    TurnContext::new(Duration::from_secs(30), Cancellation::default()).unwrap()
}
fn write_call() -> ToolCall {
    ToolCall {
        id: "write".into(),
        name: "write_file".into(),
        arguments: json!({"path":"answer.txt","content":"once"}),
    }
}
fn script(write: bool) -> ScriptedProvider {
    let mut responses = vec![];
    if write {
        responses.push(Completion {
            text: String::new(),
            tool_calls: vec![write_call()],
        });
    }
    responses.push(Completion {
        text: "done".into(),
        tool_calls: vec![],
    });
    ScriptedProvider(responses.into())
}
fn terminal(pal: &PalDefinition) -> TerminalTools {
    TerminalTools::new(
        &pal.workspace,
        ToolPolicy {
            allow_write: true,
            ..ToolPolicy::default()
        },
    )
    .unwrap()
}
#[derive(Default)]
struct Host {
    opens: usize,
    write: bool,
    fail: bool,
    routes: Vec<String>,
}
impl MailboxHost for Host {
    type Provider = ScriptedProvider;
    type Tools = TerminalTools;
    type Observer = NoopObserver;
    fn open(
        &mut self,
        pal: &PalDefinition,
        ctx: &TurnContext,
    ) -> Result<MailboxRuntime<Self::Provider, Self::Tools, Self::Observer>, AgentError> {
        ctx.remaining()?;
        self.opens += 1;
        self.routes.push(pal.route.clone());
        if self.fail {
            return Err(AgentError::Provider("fixture setup failed".into()));
        }
        Ok(MailboxRuntime {
            provider: script(self.write),
            tools: terminal(pal),
            observer: NoopObserver,
        })
    }
}

#[test]
fn duplicate_delivery_is_idempotent_before_and_after_processing_and_pause() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let p = pal(&reg, false);
    let c = Clock::new();
    let m = inbox(&reg, &p, &c);
    let message = MailMessage::operator("stable-id", "hello");
    let first = m.send(message.clone()).unwrap();
    assert!(first.inserted);
    let again = m.send(message.clone()).unwrap();
    assert!(!again.inserted);
    assert_eq!(first.task_id, again.task_id);
    let mut conflict = message.clone();
    conflict.body = "different".into();
    assert!(m.send(conflict).is_err());
    let mut host = Host::default();
    m.process_one("worker", &context(), Limits::default(), &mut host)
        .unwrap()
        .unwrap()
        .result
        .unwrap();
    assert!(
        m.process_one("worker", &context(), Limits::default(), &mut host)
            .unwrap()
            .is_none()
    );
    assert_eq!(host.opens, 1);
    reg.update(
        "owner",
        &p.id,
        1,
        PalUpdate {
            status: Some(PalStatus::Paused),
            ..PalUpdate::default()
        },
    )
    .unwrap();
    assert!(!m.send(message).unwrap().inserted);
    assert_eq!(reg.tasks("owner", &p.id).unwrap().len(), 1);
    assert!(m.send(MailMessage::operator("new-id", "hello")).is_err());
}

#[test]
fn pal_senders_require_same_owner_and_active_identity_and_cannot_spoof_duplicates() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let p = pal(&reg, false);
    let source = pal(&reg, false);
    let c = Clock::new();
    let m = inbox(&reg, &p, &c);
    let message = MailMessage {
        id: "pal-message".into(),
        sender: MailSender::Pal {
            id: source.id.clone(),
        },
        body: "review".into(),
        in_reply_to: Some("thread".into()),
    };
    m.send(message.clone()).unwrap();
    assert!(!m.send(message).unwrap().inserted);
    assert!(
        m.send(MailMessage::operator("pal-message", "review"))
            .is_err()
    );
    reg.update(
        "owner",
        &source.id,
        1,
        PalUpdate {
            status: Some(PalStatus::Paused),
            ..PalUpdate::default()
        },
    )
    .unwrap();
    assert!(
        m.send(MailMessage {
            id: "paused-source".into(),
            sender: MailSender::Pal { id: source.id },
            body: "x".into(),
            in_reply_to: None
        })
        .is_err()
    );
    let foreign = reg
        .create(PalCreate {
            owner: "other".into(),
            name: "Foreign".into(),
            purpose: String::new(),
            route: "fixture".into(),
            tools: None,
        })
        .unwrap();
    assert!(
        m.send(MailMessage {
            id: "foreign".into(),
            sender: MailSender::Pal { id: foreign.id },
            body: "x".into(),
            in_reply_to: None
        })
        .is_err()
    );
    assert!(reg.mailbox("other", &p.id).is_err());
    assert_eq!(m.messages().unwrap().len(), 1);
}

#[test]
fn automatic_batch_obeys_fifo_count_pinned_routes_and_persists_acknowledgements() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let p = pal(&reg, false);
    let c = Clock::new();
    for id in ["z-first", "a-second", "b-third"] {
        inbox(&reg, &p, &c)
            .send(MailMessage::operator(id, "hello"))
            .unwrap();
    }
    reg.update(
        "owner",
        &p.id,
        1,
        PalUpdate {
            route: Some("new-profile".into()),
            ..PalUpdate::default()
        },
    )
    .unwrap();
    drop(reg);
    let reg = registry(root.path());
    let m = inbox(&reg, &p, &c);
    let mut host = Host::default();
    let first = m
        .drain(
            "worker",
            &context(),
            BatchBudget {
                max_messages: 1,
                ..BatchBudget::default()
            },
            &mut host,
        )
        .unwrap();
    assert!(first.stopped.is_none());
    assert_eq!(first.deliveries.len(), 1);
    assert_eq!(first.deliveries[0].message_id, "z-first");
    let remaining = m
        .drain("worker", &context(), BatchBudget::default(), &mut host)
        .unwrap();
    assert_eq!(
        remaining
            .deliveries
            .iter()
            .map(|r| r.message_id.as_str())
            .collect::<Vec<_>>(),
        ["a-second", "b-third"]
    );
    assert!(remaining.deliveries.iter().all(|r| r.result.is_ok()));
    assert_eq!(host.routes, ["fixture-v1", "fixture-v1", "fixture-v1"]);
    let reopened = registry(root.path());
    assert!(
        inbox(&reopened, &p, &c)
            .messages()
            .unwrap()
            .iter()
            .all(|e| matches!(
                e.delivery,
                DeliveryState::Acknowledged {
                    outcome: TaskStatus::Complete,
                    ..
                }
            ))
    );
}

#[test]
fn expiry_before_start_reclaims_without_new_task_and_fences_all_old_token_operations() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let p = pal(&reg, false);
    let c = Clock::new();
    let m = inbox(&reg, &p, &c);
    let receipt = m.send(MailMessage::operator("msg", "hello")).unwrap();
    let first = m
        .claim("worker-a", Duration::from_millis(50))
        .unwrap()
        .unwrap();
    assert!(
        m.claim("worker-b", Duration::from_secs(1))
            .unwrap()
            .is_none()
    );
    c.set(1050);
    let second = m
        .claim("worker-b", Duration::from_secs(1))
        .unwrap()
        .unwrap();
    assert!(second.generation() > first.generation());
    assert_eq!(second.task_id(), receipt.task_id);
    assert!(m.renew(&first, Duration::from_secs(1)).is_err());
    assert!(m.release(&first).is_err());
    assert!(m.ack(&first).is_err());
    let mut host = Host::default();
    assert!(
        m.process_claimed(&first, &context(), Limits::default(), &mut host)
            .is_err()
    );
    assert_eq!(host.opens, 0);
    m.process_claimed(&second, &context(), Limits::default(), &mut host)
        .unwrap()
        .result
        .unwrap();
    assert_eq!(host.opens, 1);
    assert_eq!(reg.tasks("owner", &p.id).unwrap().len(), 1);
}

#[test]
fn renew_release_and_ack_rules_and_cross_recipient_tokens() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let p = pal(&reg, false);
    let other = pal(&reg, false);
    let c = Clock::new();
    let m = inbox(&reg, &p, &c);
    m.send(MailMessage::operator("msg", "hello")).unwrap();
    let lease = m
        .claim("worker", Duration::from_millis(50))
        .unwrap()
        .unwrap();
    assert!(m.ack(&lease).is_err());
    assert!(
        inbox(&reg, &other, &c)
            .renew(&lease, Duration::from_secs(1))
            .is_err()
    );
    assert!(reg.start_task("owner", &p.id, lease.task_id()).is_err());
    assert!(
        reg.acknowledge_interruption("owner", &p.id, lease.task_id())
            .is_err()
    );
    c.set(1040);
    m.renew(&lease, Duration::from_millis(100)).unwrap();
    c.set(1080);
    m.recover().unwrap();
    assert!(matches!(
        m.messages().unwrap()[0].delivery,
        DeliveryState::Leased { .. }
    ));
    m.release(&lease).unwrap();
    let current = m.claim("worker", Duration::from_secs(1)).unwrap().unwrap();
    assert!(m.release(&lease).is_err());
    m.process_claimed(
        &current,
        &context(),
        Limits::default(),
        &mut Host::default(),
    )
    .unwrap()
    .result
    .unwrap();
    c.set(100000);
    assert!(matches!(
        m.ack(&current).unwrap(),
        DeliveryState::Acknowledged { .. }
    ));
}

#[test]
fn invalid_clock_message_and_aggregate_budgets_do_no_work() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let p = pal(&reg, false);
    let c = Clock::new();
    let m = inbox(&reg, &p, &c);
    for message in [
        MailMessage::operator("../bad", "x"),
        MailMessage::operator("empty", ""),
        MailMessage::operator("large", "x".repeat(16 * 1024 + 1)),
    ] {
        assert!(m.send(message).is_err());
    }
    m.send(MailMessage::operator("msg", "hello")).unwrap();
    assert!(m.claim("../worker", Duration::from_secs(1)).is_err());
    assert!(m.claim("worker", Duration::ZERO).is_err());
    assert!(m.claim("worker", Duration::from_secs(86401)).is_err());
    c.set(u64::MAX);
    assert!(m.claim("worker", Duration::from_secs(1)).is_err());
    c.set(1000);
    let mut host = Host::default();
    for budget in [
        BatchBudget {
            max_messages: 0,
            ..BatchBudget::default()
        },
        BatchBudget {
            max_messages: 65,
            ..BatchBudget::default()
        },
        BatchBudget {
            max_messages: 8,
            ..BatchBudget::default()
        },
        BatchBudget {
            per_message: Limits {
                max_text_bytes: usize::MAX,
                ..Limits::default()
            },
            ..BatchBudget::default()
        },
    ] {
        assert!(m.drain("worker", &context(), budget, &mut host).is_err());
    }
    let ctx = context();
    ctx.cancellation().cancel();
    assert!(
        m.process_one("worker", &ctx, Limits::default(), &mut host)
            .is_err()
    );
    assert_eq!(host.opens, 0);
    assert_eq!(m.messages().unwrap()[0].generation, 0);
}

#[test]
fn factory_failure_is_terminal_and_does_not_keep_execution_lock_or_retry() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let p = pal(&reg, false);
    let c = Clock::new();
    let m = inbox(&reg, &p, &c);
    m.send(MailMessage::operator("bad", "hello")).unwrap();
    let mut host = Host {
        fail: true,
        ..Host::default()
    };
    let result = m
        .process_one("worker", &context(), Limits::default(), &mut host)
        .unwrap()
        .unwrap();
    assert!(result.result.is_err());
    assert!(matches!(
        result.delivery,
        DeliveryState::Acknowledged {
            outcome: TaskStatus::Failed,
            ..
        }
    ));
    host.fail = false;
    m.send(MailMessage::operator("good", "hello")).unwrap();
    m.process_one("worker", &context(), Limits::default(), &mut host)
        .unwrap()
        .unwrap()
        .result
        .unwrap();
    assert_eq!(host.opens, 2);
    assert!(
        m.process_one("worker", &context(), Limits::default(), &mut host)
            .unwrap()
            .is_none()
    );
}

#[test]
fn expired_live_worker_cannot_be_taken_over_and_cannot_start_a_tool_effect() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let p = pal(&reg, true);
    let c = Clock::new();
    let m = inbox(&reg, &p, &c);
    m.send(MailMessage::operator("msg", "write")).unwrap();
    let lease = m
        .claim("first", Duration::from_millis(50))
        .unwrap()
        .unwrap();
    struct DuringProvider<'a> {
        mailbox: &'a Mailbox<'a, Clock>,
        clock: Clock,
    }
    impl Provider for DuringProvider<'_> {
        fn complete(
            &mut self,
            _: &[Message],
            _: &[ToolDefinition],
            _: &TurnContext,
        ) -> Result<Completion, AgentError> {
            self.clock.set(2000);
            assert!(
                self.mailbox
                    .claim("takeover", Duration::from_secs(1))
                    .is_err()
            );
            assert!(self.mailbox.recover().is_err());
            Ok(Completion {
                text: String::new(),
                tool_calls: vec![write_call()],
            })
        }
    }
    struct LiveHost<'a> {
        mailbox: &'a Mailbox<'a, Clock>,
        clock: Clock,
    }
    impl<'a> MailboxHost for LiveHost<'a> {
        type Provider = DuringProvider<'a>;
        type Tools = TerminalTools;
        type Observer = NoopObserver;
        fn open(
            &mut self,
            pal: &PalDefinition,
            _: &TurnContext,
        ) -> Result<MailboxRuntime<Self::Provider, Self::Tools, Self::Observer>, AgentError>
        {
            Ok(MailboxRuntime {
                provider: DuringProvider {
                    mailbox: self.mailbox,
                    clock: self.clock.clone(),
                },
                tools: terminal(pal),
                observer: NoopObserver,
            })
        }
    }
    let result = m
        .process_claimed(
            &lease,
            &context(),
            Limits::default(),
            &mut LiveHost {
                mailbox: &m,
                clock: c,
            },
        )
        .unwrap();
    assert!(result.result.is_err());
    assert!(!p.workspace.join("answer.txt").exists());
    assert!(matches!(
        result.delivery,
        DeliveryState::Acknowledged {
            outcome: TaskStatus::Failed,
            ..
        }
    ));
}

#[test]
fn expiry_after_pending_checkpoint_needs_inspection_and_blocks_next_delivery() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let p = pal(&reg, true);
    let c = Clock::new();
    let m = inbox(&reg, &p, &c);
    let first = m.send(MailMessage::operator("first", "write")).unwrap();
    m.send(MailMessage::operator("next", "write")).unwrap();
    let lease = m
        .claim("worker", Duration::from_millis(50))
        .unwrap()
        .unwrap();
    struct Expire(Clock);
    impl Observer for Expire {
        fn event(&mut self, event: Event) -> Result<(), AgentError> {
            if matches!(event, Event::ToolStarted { .. }) {
                self.0.set(2000);
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
    struct ExpireHost(Clock);
    impl MailboxHost for ExpireHost {
        type Provider = ScriptedProvider;
        type Tools = TerminalTools;
        type Observer = Expire;
        fn open(
            &mut self,
            pal: &PalDefinition,
            _: &TurnContext,
        ) -> Result<MailboxRuntime<Self::Provider, Self::Tools, Self::Observer>, AgentError>
        {
            Ok(MailboxRuntime {
                provider: script(true),
                tools: terminal(pal),
                observer: Expire(self.0.clone()),
            })
        }
    }
    let result = m
        .process_claimed(
            &lease,
            &context(),
            Limits::default(),
            &mut ExpireHost(c.clone()),
        )
        .unwrap();
    assert!(result.result.is_err());
    assert!(matches!(
        result.delivery,
        DeliveryState::NeedsInspection { .. }
    ));
    assert!(!p.workspace.join("answer.txt").exists());
    assert!(
        reg.session("owner", &p.id, &first.task_id)
            .unwrap()
            .pending_call
            .is_some()
    );
    assert!(m.claim("other", Duration::from_secs(1)).unwrap().is_none());
    assert!(m.ack(&lease).is_err());
    m.resolve_inspection("first").unwrap();
    assert_eq!(
        reg.session("owner", &p.id, &first.task_id).unwrap().status,
        SessionStatus::Failed
    );
    assert!(
        reg.session("owner", &p.id, &first.task_id)
            .unwrap()
            .pending_call
            .is_some()
    );
    let mut host = Host {
        write: true,
        ..Host::default()
    };
    m.process_one("worker", &context(), Limits::default(), &mut host)
        .unwrap()
        .unwrap()
        .result
        .unwrap();
    assert_eq!(host.opens, 1);
    assert_eq!(
        std::fs::read_to_string(p.workspace.join("answer.txt")).unwrap(),
        "once"
    );
}

#[test]
fn paused_claim_remains_pristine_and_can_resume_only_after_unpause() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let p = pal(&reg, false);
    let c = Clock::new();
    let m = inbox(&reg, &p, &c);
    m.send(MailMessage::operator("msg", "hello")).unwrap();
    let lease = m
        .claim("worker", Duration::from_millis(50))
        .unwrap()
        .unwrap();
    reg.update(
        "owner",
        &p.id,
        1,
        PalUpdate {
            status: Some(PalStatus::Paused),
            ..PalUpdate::default()
        },
    )
    .unwrap();
    let mut host = Host::default();
    assert!(
        m.process_claimed(&lease, &context(), Limits::default(), &mut host)
            .is_err()
    );
    assert_eq!(host.opens, 0);
    c.set(1050);
    m.recover().unwrap();
    assert_eq!(m.messages().unwrap()[0].delivery, DeliveryState::Ready);
    assert!(m.claim("worker", Duration::from_secs(1)).is_err());
    reg.update(
        "owner",
        &p.id,
        2,
        PalUpdate {
            status: Some(PalStatus::Active),
            ..PalUpdate::default()
        },
    )
    .unwrap();
    m.process_one("worker", &context(), Limits::default(), &mut host)
        .unwrap()
        .unwrap()
        .result
        .unwrap();
}

fn child(root: &Path, id: &str, mode: &str) -> std::process::Output {
    std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "mailbox_subprocess_fixture", "--nocapture"])
        .env("NAMZU_MAIL_FIXTURE_ROOT", root)
        .env("NAMZU_MAIL_FIXTURE_PAL", id)
        .env("NAMZU_MAIL_FIXTURE_MODE", mode)
        .output()
        .unwrap()
}
#[test]
fn mailbox_subprocess_fixture() {
    let Some(root) = std::env::var_os("NAMZU_MAIL_FIXTURE_ROOT") else {
        return;
    };
    let id = std::env::var("NAMZU_MAIL_FIXTURE_PAL").unwrap();
    let mode = std::env::var("NAMZU_MAIL_FIXTURE_MODE").unwrap();
    let reg = registry(Path::new(&root));
    let p = reg.get("owner", &id).unwrap();
    let c = Clock::new();
    let m = inbox(&reg, &p, &c);
    if mode == "publish" {
        struct ExitClock;
        impl MailboxClock for ExitClock {
            fn now_ms(&self) -> u64 {
                std::process::exit(0)
            }
        }
        // Clock read occurs after the private session write, before manifest publication.
        m.with_clock(ExitClock)
            .send(MailMessage::operator("msg", "hello"))
            .unwrap();
        panic!("fixture must exit before publishing message/task references")
    }
    if mode == "provider" {
        struct ExitProvider;
        impl Provider for ExitProvider {
            fn complete(
                &mut self,
                _: &[Message],
                _: &[ToolDefinition],
                _: &TurnContext,
            ) -> Result<Completion, AgentError> {
                std::process::exit(0)
            }
        }
        struct ExitHost;
        impl MailboxHost for ExitHost {
            type Provider = ExitProvider;
            type Tools = TerminalTools;
            type Observer = NoopObserver;
            fn open(
                &mut self,
                pal: &PalDefinition,
                _: &TurnContext,
            ) -> Result<MailboxRuntime<Self::Provider, Self::Tools, Self::Observer>, AgentError>
            {
                Ok(MailboxRuntime {
                    provider: ExitProvider,
                    tools: terminal(pal),
                    observer: NoopObserver,
                })
            }
        }
        m.process_one("crashed", &context(), Limits::default(), &mut ExitHost)
            .unwrap();
        panic!("fixture must exit inside provider without a tool checkpoint")
    }
    if mode == "claim" {
        m.claim("crashed", Duration::from_millis(50))
            .unwrap()
            .unwrap();
        std::process::exit(0);
    }
    if mode == "complete" {
        struct ExitAfterFinish;
        impl Observer for ExitAfterFinish {
            fn checkpoint(
                &mut self,
                _: &[Message],
                _: &Stats,
                _: Option<&ToolCall>,
            ) -> Result<(), AgentError> {
                Ok(())
            }
        }
        impl Drop for ExitAfterFinish {
            fn drop(&mut self) {
                std::process::exit(0);
            }
        }
        struct FinishHost;
        impl MailboxHost for FinishHost {
            type Provider = ScriptedProvider;
            type Tools = TerminalTools;
            type Observer = ExitAfterFinish;
            fn open(
                &mut self,
                pal: &PalDefinition,
                _: &TurnContext,
            ) -> Result<MailboxRuntime<Self::Provider, Self::Tools, Self::Observer>, AgentError>
            {
                Ok(MailboxRuntime {
                    provider: script(false),
                    tools: terminal(pal),
                    observer: ExitAfterFinish,
                })
            }
        }
        // Runtime fields drop after PalRun has persisted Complete, before ack.
        m.process_one("crashed", &context(), Limits::default(), &mut FinishHost)
            .unwrap();
        panic!("fixture must exit between finish and acknowledgement")
    }
    struct CrashTools {
        inner: TerminalTools,
        path: std::path::PathBuf,
    }
    impl Tools for CrashTools {
        fn definitions(&self) -> Vec<ToolDefinition> {
            self.inner.definitions()
        }
        fn call(&mut self, call: &ToolCall, ctx: &TurnContext) -> Result<ToolResult, AgentError> {
            let result = self.inner.call(call, ctx)?;
            assert!(!result.is_error);
            use std::io::Write;
            let mut audit = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
                .unwrap();
            audit.write_all(b"effect\n").unwrap();
            audit.sync_all().unwrap();
            std::process::exit(0)
        }
    }
    struct CrashHost;
    impl MailboxHost for CrashHost {
        type Provider = ScriptedProvider;
        type Tools = CrashTools;
        type Observer = NoopObserver;
        fn open(
            &mut self,
            pal: &PalDefinition,
            _: &TurnContext,
        ) -> Result<MailboxRuntime<Self::Provider, Self::Tools, Self::Observer>, AgentError>
        {
            Ok(MailboxRuntime {
                provider: script(true),
                tools: CrashTools {
                    inner: terminal(pal),
                    path: pal.workspace.join("audit.txt"),
                },
                observer: NoopObserver,
            })
        }
    }
    m.process_one("crashed", &context(), Limits::default(), &mut CrashHost)
        .unwrap();
    panic!("fixture should exit inside effect")
}

#[test]
fn real_crash_before_start_can_recover_same_delivery_without_duplicate_task() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let p = pal(&reg, false);
    let c = Clock::new();
    let m = inbox(&reg, &p, &c);
    let receipt = m.send(MailMessage::operator("msg", "hello")).unwrap();
    let output = child(root.path(), &p.id, "claim");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    c.set(1050);
    let reopened = registry(root.path());
    let m = inbox(&reopened, &p, &c);
    let mut host = Host::default();
    let result = m
        .process_one("replacement", &context(), Limits::default(), &mut host)
        .unwrap()
        .unwrap();
    assert_eq!(result.task_id, receipt.task_id);
    result.result.unwrap();
    assert_eq!(reopened.tasks("owner", &p.id).unwrap().len(), 1);
    assert_eq!(host.opens, 1);
}

#[test]
fn real_crash_after_effect_is_quarantined_and_never_replayed() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let p = pal(&reg, true);
    let c = Clock::new();
    let m = inbox(&reg, &p, &c);
    let message = MailMessage::operator("msg", "write");
    let receipt = m.send(message.clone()).unwrap();
    let output = child(root.path(), &p.id, "effect");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(p.workspace.join("audit.txt")).unwrap(),
        "effect\n"
    );
    c.set(1_000_000);
    let reopened = registry(root.path());
    let m = inbox(&reopened, &p, &c);
    m.recover().unwrap();
    assert!(matches!(
        m.messages().unwrap()[0].delivery,
        DeliveryState::NeedsInspection { .. }
    ));
    let session = reopened.session("owner", &p.id, &receipt.task_id).unwrap();
    assert_eq!(session.status, SessionStatus::Running);
    assert!(session.pending_call.is_some());
    assert!(!m.send(message).unwrap().inserted);
    let mut host = Host {
        write: true,
        ..Host::default()
    };
    assert!(
        m.process_one("worker", &context(), Limits::default(), &mut host)
            .unwrap()
            .is_none()
    );
    assert_eq!(host.opens, 0);
    m.resolve_inspection("msg").unwrap();
    assert!(m.claim("worker", Duration::from_secs(1)).unwrap().is_none());
    assert_eq!(
        std::fs::read_to_string(p.workspace.join("audit.txt")).unwrap(),
        "effect\n"
    );
}

#[test]
fn completion_before_ack_is_reconciled_without_invoking_model_again() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let p = pal(&reg, false);
    let c = Clock::new();
    let m = inbox(&reg, &p, &c);
    let receipt = m.send(MailMessage::operator("msg", "hello")).unwrap();
    let output = child(root.path(), &p.id, "complete");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(matches!(
        m.messages().unwrap()[0].delivery,
        DeliveryState::Leased { .. }
    ));
    assert_eq!(
        reg.tasks("owner", &p.id).unwrap()[0].status,
        TaskStatus::Complete
    );
    let session = reg.session("owner", &p.id, &receipt.task_id).unwrap();
    assert!(
        matches!(session.messages.last(),Some(Message::Assistant{content,..}) if content=="done")
    );
    c.set(1_000_000);
    m.recover().unwrap();
    assert!(matches!(
        m.messages().unwrap()[0].delivery,
        DeliveryState::Acknowledged {
            outcome: TaskStatus::Complete,
            ..
        }
    ));
    let mut host = Host::default();
    assert!(
        m.process_one("worker", &context(), Limits::default(), &mut host)
            .unwrap()
            .is_none()
    );
    assert_eq!(host.opens, 0);
}

#[test]
fn old_registry_without_mailbox_field_loads_and_bad_mail_binding_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let p = pal(&reg, false);
    let c = Clock::new();
    let store = PrivateStore::open(root.path().join("state")).unwrap();
    let name = format!("{}.json", p.id);
    let mut record: serde_json::Value = store.read(&name).unwrap().unwrap();
    record.as_object_mut().unwrap().remove("mailbox");
    store.write(&name, &record).unwrap();
    let m = inbox(&reg, &p, &c);
    m.send(MailMessage::operator("msg", "hello")).unwrap();
    let mut record: serde_json::Value = store.read(&name).unwrap().unwrap();
    record["mailbox"][0]["task_id"] = json!("missing-task");
    store.write(&name, &record).unwrap();
    assert!(m.claim("worker", Duration::from_secs(1)).is_err());
}

#[test]
fn auto_worker_preserves_profile_ceiling_and_per_call_approval() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let c = Clock::new();
    struct Deny(Arc<AtomicU64>);
    impl Observer for Deny {
        fn approve(&mut self, _: &ToolCall, _: &TurnContext) -> Result<bool, AgentError> {
            self.0.fetch_add(1, Ordering::SeqCst);
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
    struct DenyHost(Arc<AtomicU64>);
    impl MailboxHost for DenyHost {
        type Provider = ScriptedProvider;
        type Tools = TerminalTools;
        type Observer = Deny;
        fn open(
            &mut self,
            pal: &PalDefinition,
            _: &TurnContext,
        ) -> Result<MailboxRuntime<Self::Provider, Self::Tools, Self::Observer>, AgentError>
        {
            Ok(MailboxRuntime {
                provider: script(true),
                tools: terminal(pal),
                observer: Deny(self.0.clone()),
            })
        }
    }
    let approvals = Arc::new(AtomicU64::new(0));
    let mut host = DenyHost(approvals.clone());
    for (grant, expected) in [(false, 0), (true, 1)] {
        let p = pal(&reg, grant);
        let m = inbox(&reg, &p, &c);
        m.send(MailMessage::operator("msg", "write")).unwrap();
        let turn = m
            .process_one("worker", &context(), Limits::default(), &mut host)
            .unwrap()
            .unwrap()
            .result
            .unwrap();
        assert!(
            turn.messages
                .iter()
                .any(|m| matches!(m, Message::Tool { is_error: true, .. }))
        );
        assert!(!p.workspace.join("answer.txt").exists());
        assert_eq!(approvals.load(Ordering::SeqCst), expected);
    }
}

#[test]
fn batch_cancel_keeps_finished_receipt_and_leaves_unstarted_message_ready() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let p = pal(&reg, false);
    let c = Clock::new();
    let m = inbox(&reg, &p, &c);
    m.send(MailMessage::operator("first", "x")).unwrap();
    m.send(MailMessage::operator("second", "x")).unwrap();
    struct CancelProvider(Cancellation);
    impl Provider for CancelProvider {
        fn complete(
            &mut self,
            _: &[Message],
            _: &[ToolDefinition],
            _: &TurnContext,
        ) -> Result<Completion, AgentError> {
            self.0.cancel();
            Ok(Completion {
                text: "cancelled".into(),
                tool_calls: vec![],
            })
        }
    }
    struct CancelHost {
        token: Cancellation,
        opens: usize,
    }
    impl MailboxHost for CancelHost {
        type Provider = CancelProvider;
        type Tools = TerminalTools;
        type Observer = NoopObserver;
        fn open(
            &mut self,
            pal: &PalDefinition,
            _: &TurnContext,
        ) -> Result<MailboxRuntime<Self::Provider, Self::Tools, Self::Observer>, AgentError>
        {
            self.opens += 1;
            Ok(MailboxRuntime {
                provider: CancelProvider(self.token.clone()),
                tools: terminal(pal),
                observer: NoopObserver,
            })
        }
    }
    let ctx = context();
    let mut host = CancelHost {
        token: ctx.cancellation(),
        opens: 0,
    };
    let batch = m
        .drain("worker", &ctx, BatchBudget::default(), &mut host)
        .unwrap();
    assert_eq!(batch.stopped, Some(AgentError::Cancelled));
    assert_eq!(batch.deliveries.len(), 1);
    assert_eq!(
        batch.deliveries[0].result.as_ref().unwrap_err(),
        &AgentError::Cancelled
    );
    assert_eq!(host.opens, 1);
    let messages = m.messages().unwrap();
    assert!(matches!(
        messages[0].delivery,
        DeliveryState::Acknowledged {
            outcome: TaskStatus::Cancelled,
            ..
        }
    ));
    assert_eq!(messages[1].delivery, DeliveryState::Ready);
    assert_eq!(messages[1].generation, 0);
}

#[test]
fn panicking_provider_is_terminal_and_never_scheduled_again() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let p = pal(&reg, false);
    let c = Clock::new();
    let m = inbox(&reg, &p, &c);
    let receipt = m.send(MailMessage::operator("msg", "hello")).unwrap();
    struct Panics;
    impl Provider for Panics {
        fn complete(
            &mut self,
            _: &[Message],
            _: &[ToolDefinition],
            _: &TurnContext,
        ) -> Result<Completion, AgentError> {
            panic!("fixture provider panic")
        }
    }
    struct PanicHost;
    impl MailboxHost for PanicHost {
        type Provider = Panics;
        type Tools = TerminalTools;
        type Observer = NoopObserver;
        fn open(
            &mut self,
            pal: &PalDefinition,
            _: &TurnContext,
        ) -> Result<MailboxRuntime<Self::Provider, Self::Tools, Self::Observer>, AgentError>
        {
            Ok(MailboxRuntime {
                provider: Panics,
                tools: terminal(pal),
                observer: NoopObserver,
            })
        }
    }
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        m.process_one("worker", &context(), Limits::default(), &mut PanicHost)
    }));
    assert!(failed.is_err());
    assert_eq!(
        reg.tasks("owner", &p.id).unwrap()[0].status,
        TaskStatus::Interrupted
    );
    c.set(1_000_000);
    m.recover().unwrap();
    // Dropped lease is durably Interrupted: it is terminal and never scheduled again.
    assert!(matches!(
        m.messages().unwrap()[0].delivery,
        DeliveryState::Acknowledged {
            outcome: TaskStatus::Interrupted,
            ..
        }
    ));
    assert!(
        reg.session("owner", &p.id, &receipt.task_id)
            .unwrap()
            .pending_call
            .is_none()
    );
    assert!(
        m.claim("replacement", Duration::from_secs(1))
            .unwrap()
            .is_none()
    );
}

#[test]
fn real_crash_during_provider_without_pending_tool_still_blocks_automatic_retry() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let p = pal(&reg, false);
    let c = Clock::new();
    let m = inbox(&reg, &p, &c);
    let receipt = m.send(MailMessage::operator("msg", "hello")).unwrap();
    let output = child(root.path(), &p.id, "provider");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        reg.tasks("owner", &p.id).unwrap()[0].status,
        TaskStatus::Running
    );
    assert!(
        reg.session("owner", &p.id, &receipt.task_id)
            .unwrap()
            .pending_call
            .is_none()
    );
    c.set(1_000_000);
    m.recover().unwrap();
    assert!(matches!(
        m.messages().unwrap()[0].delivery,
        DeliveryState::NeedsInspection { .. }
    ));
    let mut host = Host::default();
    assert!(
        m.process_one("replacement", &context(), Limits::default(), &mut host)
            .unwrap()
            .is_none()
    );
    assert_eq!(host.opens, 0);
}

#[test]
fn crash_before_atomic_message_task_publication_leaves_no_executable_duplicate() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let p = pal(&reg, false);
    let c = Clock::new();
    let m = inbox(&reg, &p, &c);
    let output = child(root.path(), &p.id, "publish");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(m.messages().unwrap().is_empty());
    assert!(reg.tasks("owner", &p.id).unwrap().is_empty());
    let private = PrivateStore::open(root.path().join("state"))
        .unwrap()
        .open_child(&p.id)
        .unwrap();
    assert_eq!(
        private
            .names()
            .unwrap()
            .iter()
            .filter(|n| n.ends_with(".json"))
            .count(),
        1
    ); // unreferenced session
    let message = MailMessage::operator("msg", "hello");
    assert!(m.send(message.clone()).unwrap().inserted);
    assert!(!m.send(message).unwrap().inserted);
    let mut host = Host::default();
    let batch = m
        .drain("worker", &context(), BatchBudget::default(), &mut host)
        .unwrap();
    assert_eq!(batch.deliveries.len(), 1);
    assert_eq!(host.opens, 1);
    assert_eq!(reg.tasks("owner", &p.id).unwrap().len(), 1);
}

#[test]
fn pause_from_tool_started_callback_prevents_write_and_preserves_pending_inspection() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let p = pal(&reg, true);
    let c = Clock::new();
    let m = inbox(&reg, &p, &c);
    m.send(MailMessage::operator("msg", "write")).unwrap();
    struct Pause<'a> {
        registry: &'a Registry,
        id: String,
    }
    impl Observer for Pause<'_> {
        fn event(&mut self, event: Event) -> Result<(), AgentError> {
            if matches!(event, Event::ToolStarted { .. }) {
                self.registry.update(
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
    struct PauseHost<'a>(&'a Registry);
    impl<'a> MailboxHost for PauseHost<'a> {
        type Provider = ScriptedProvider;
        type Tools = TerminalTools;
        type Observer = Pause<'a>;
        fn open(
            &mut self,
            pal: &PalDefinition,
            _: &TurnContext,
        ) -> Result<MailboxRuntime<Self::Provider, Self::Tools, Self::Observer>, AgentError>
        {
            Ok(MailboxRuntime {
                provider: script(true),
                tools: terminal(pal),
                observer: Pause {
                    registry: self.0,
                    id: pal.id.clone(),
                },
            })
        }
    }
    let delivery = m
        .process_one(
            "worker",
            &context(),
            Limits::default(),
            &mut PauseHost(&reg),
        )
        .unwrap()
        .unwrap();
    assert!(delivery.result.is_err());
    assert!(!p.workspace.join("answer.txt").exists());
    assert!(matches!(
        delivery.delivery,
        DeliveryState::NeedsInspection { .. }
    ));
}

#[test]
fn sender_metadata_is_never_promoted_to_system_instructions() {
    let root = tempfile::tempdir().unwrap();
    let reg = registry(root.path());
    let p = pal(&reg, false);
    let source = pal(&reg, false);
    let c = Clock::new();
    let m = inbox(&reg, &p, &c);
    m.send(MailMessage {
        id: "msg".into(),
        sender: MailSender::Pal { id: source.id },
        body: "untrusted task".into(),
        in_reply_to: None,
    })
    .unwrap();
    struct Inspect;
    impl Provider for Inspect {
        fn complete(
            &mut self,
            messages: &[Message],
            _: &[ToolDefinition],
            _: &TurnContext,
        ) -> Result<Completion, AgentError> {
            assert_eq!(
                messages,
                &[Message::User {
                    content: "untrusted task".into()
                }]
            );
            Ok(Completion {
                text: "ok".into(),
                tool_calls: vec![],
            })
        }
    }
    struct InspectHost;
    impl MailboxHost for InspectHost {
        type Provider = Inspect;
        type Tools = TerminalTools;
        type Observer = NoopObserver;
        fn open(
            &mut self,
            pal: &PalDefinition,
            _: &TurnContext,
        ) -> Result<MailboxRuntime<Self::Provider, Self::Tools, Self::Observer>, AgentError>
        {
            Ok(MailboxRuntime {
                provider: Inspect,
                tools: terminal(pal),
                observer: NoopObserver,
            })
        }
    }
    m.process_one("worker", &context(), Limits::default(), &mut InspectHost)
        .unwrap()
        .unwrap()
        .result
        .unwrap();
}
