use namzu_core::{
    AgentError, Cancellation, Completion, Limits, Message, NoopObserver, Provider,
    ScriptedProvider, ToolCall, ToolDefinition, ToolResult, Tools, TurnContext,
};
use namzu_pals::{Job, TaskSpec, Team};
use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

struct NoTools;
impl Tools for NoTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![]
    }
    fn call(&mut self, _: &ToolCall, _: &TurnContext) -> Result<ToolResult, AgentError> {
        panic!("no tools")
    }
}
fn context() -> TurnContext {
    TurnContext::new(Duration::from_secs(30), Cancellation::default()).unwrap()
}
fn spec(id: &str) -> TaskSpec {
    let mut s = TaskSpec::new(id, id);
    s.limits.max_steps = 2;
    s.limits.max_tool_calls = 0;
    s.limits.max_text_bytes = 4096;
    s
}
fn scripted() -> ScriptedProvider {
    ScriptedProvider(
        vec![Completion {
            text: "ok".into(),
            tool_calls: vec![],
        }]
        .into(),
    )
}
type Gate = Arc<(Mutex<bool>, Condvar)>;
struct Waiting {
    id: String,
    started: mpsc::Sender<String>,
    gate: Gate,
    active: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}
struct Active(Arc<AtomicUsize>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
impl Provider for Waiting {
    fn complete(
        &mut self,
        _: &[Message],
        _: &[ToolDefinition],
        ctx: &TurnContext,
    ) -> Result<Completion, AgentError> {
        let n = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        let _active = Active(self.active.clone());
        self.peak.fetch_max(n, Ordering::SeqCst);
        self.started.send(self.id.clone()).unwrap();
        let (lock, cv) = &*self.gate;
        let mut released = lock.lock().unwrap();
        while !*released {
            ctx.remaining()?;
            released = cv
                .wait_timeout(released, Duration::from_millis(10))
                .unwrap()
                .0;
        }
        ctx.remaining()?;
        Ok(Completion {
            text: self.id.clone(),
            tool_calls: vec![],
        })
    }
}
fn release(gate: &Gate) {
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
}

#[test]
fn bounded_workers_really_overlap_and_results_keep_submission_order() {
    let ctx = context();
    let team = Team::new("parent", &ctx, Limits::default(), &NoTools, 2).unwrap();
    let (tx, rx) = mpsc::channel();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let jobs = (0..8)
        .map(|i| {
            let id = format!("job-{i}");
            Job::new(
                team.prepare(spec(&id)).unwrap(),
                Waiting {
                    id,
                    started: tx.clone(),
                    gate: gate.clone(),
                    active: active.clone(),
                    peak: peak.clone(),
                },
                NoTools,
                NoopObserver,
            )
        })
        .collect();
    let handle = std::thread::spawn(move || team.run(jobs).unwrap());
    // Both providers must enter before either is released: serial code cannot pass.
    let first = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let second = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_ne!(first, second);
    assert!(rx.try_recv().is_err());
    release(&gate);
    let results = handle.join().unwrap();
    assert_eq!(peak.load(Ordering::SeqCst), 2);
    assert_eq!(active.load(Ordering::SeqCst), 0); // return means workers finished
    for (i, r) in results.into_iter().enumerate() {
        assert_eq!(r.id, format!("job-{i}"));
        assert_eq!(r.result.unwrap().output, r.id);
    }
}

#[test]
fn owner_cancel_stops_active_and_queued_jobs_and_joins_worker() {
    let ctx = context();
    let team = Team::new("owner", &ctx, Limits::default(), &NoTools, 1).unwrap();
    let control = team.control();
    let (tx, rx) = mpsc::channel();
    let active = Arc::new(AtomicUsize::new(0));
    let jobs = (0..3)
        .map(|i| {
            let id = format!("job-{i}");
            Job::new(
                team.prepare(spec(&id)).unwrap(),
                Waiting {
                    id,
                    started: tx.clone(),
                    gate: Arc::new((Mutex::new(false), Condvar::new())),
                    active: active.clone(),
                    peak: Arc::default(),
                },
                NoTools,
                NoopObserver,
            )
        })
        .collect();
    assert!(control.cancel("intruder").is_err());
    assert!(control.cancel_task("intruder", "job-0").is_err());
    let handle = std::thread::spawn(move || team.run(jobs).unwrap());
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "job-0");
    control.cancel("owner").unwrap();
    for r in handle.join().unwrap() {
        assert_eq!(r.result.unwrap_err(), AgentError::Cancelled);
    }
    assert!(rx.try_recv().is_err());
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert!(ctx.remaining().is_ok());
}

#[test]
fn individual_cancel_leaves_sibling_and_other_team_alive() {
    let ctx = context();
    let team = Team::new("owner", &ctx, Limits::default(), &NoTools, 2).unwrap();
    let a = team.prepare(spec("a")).unwrap();
    let b = team.prepare(spec("b")).unwrap();
    team.control().cancel_task("owner", "a").unwrap();
    assert!(team.control().cancel_task("owner", "missing").is_err());
    let other = Team::new("owner", &ctx, Limits::default(), &NoTools, 1).unwrap();
    let results = team
        .run(vec![
            Job::new(a, scripted(), NoTools, NoopObserver),
            Job::new(b, scripted(), NoTools, NoopObserver),
        ])
        .unwrap();
    assert_eq!(
        results[0].result.as_ref().unwrap_err(),
        &AgentError::Cancelled
    );
    assert!(results[1].result.is_ok());
    let c = other.prepare(spec("a")).unwrap();
    assert!(
        other
            .run(vec![Job::new(c, scripted(), NoTools, NoopObserver)])
            .unwrap()[0]
            .result
            .is_ok()
    );
}

#[test]
fn foreign_job_rejected_before_any_provider_invocation() {
    let ctx = context();
    let a = Team::new("owner", &ctx, Limits::default(), &NoTools, 1).unwrap();
    let b = Team::new("owner", &ctx, Limits::default(), &NoTools, 1).unwrap();
    struct Never;
    impl Provider for Never {
        fn complete(
            &mut self,
            _: &[Message],
            _: &[ToolDefinition],
            _: &TurnContext,
        ) -> Result<Completion, AgentError> {
            panic!("must reject before provider")
        }
    }
    let alien = Job::new(
        b.prepare(spec("alien")).unwrap(),
        Never,
        NoTools,
        NoopObserver,
    );
    assert!(a.run(vec![alien]).is_err());
    assert!(b.prepare(spec("still-live")).is_ok());
}

#[test]
fn parent_cancel_reaches_all_teams_and_drop_cancels_unstarted_children() {
    let ctx = context();
    let a = Team::new("owner", &ctx, Limits::default(), &NoTools, 1).unwrap();
    let task = a.prepare(spec("a")).unwrap();
    let token = task.cancellation();
    drop(a);
    assert!(token.is_cancelled());
    let b = Team::new("owner", &ctx, Limits::default(), &NoTools, 1).unwrap();
    let task = b.prepare(spec("b")).unwrap();
    ctx.cancellation().cancel();
    assert_eq!(
        b.run(vec![Job::new(task, scripted(), NoTools, NoopObserver)])
            .unwrap()[0]
            .result
            .as_ref()
            .unwrap_err(),
        &AgentError::Cancelled
    );
}

#[test]
fn panicking_job_is_failed_without_losing_other_results() {
    struct Panics;
    impl Provider for Panics {
        fn complete(
            &mut self,
            _: &[Message],
            _: &[ToolDefinition],
            _: &TurnContext,
        ) -> Result<Completion, AgentError> {
            panic!("fixture panic")
        }
    }
    let ctx = context();
    let team = Team::new("owner", &ctx, Limits::default(), &NoTools, 1).unwrap();
    let jobs = vec![
        Job::new(
            team.prepare(spec("bad")).unwrap(),
            Panics,
            NoTools,
            NoopObserver,
        ),
        Job::new(
            team.prepare(spec("good")).unwrap(),
            scripted(),
            NoTools,
            NoopObserver,
        ),
    ];
    let results = team.run(jobs).unwrap();
    assert!(matches!(
        results[0].result,
        Err(AgentError::State("child runtime panicked"))
    ));
    assert!(results[1].result.is_ok());
}

#[test]
fn concurrency_bounds_and_empty_team() {
    let ctx = context();
    for n in [0, 17] {
        assert!(Team::new("owner", &ctx, Limits::default(), &NoTools, n).is_err());
    }
    assert!(
        Team::new("owner", &ctx, Limits::default(), &NoTools, 1)
            .unwrap()
            .run(vec![])
            .unwrap()
            .is_empty()
    );
}
