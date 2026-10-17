use crate::{ChildTask, Delegation, TaskResult, TaskSpec};
use namzu_core::{AgentError, Cancellation, Limits, Observer, Provider, Tools, Turn, TurnContext};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex},
    thread,
};

/// One owned runtime per job; providers, tools and journals are never shared implicitly.
pub struct Job {
    scope: u64,
    id: String,
    owner: String,
    cancellation: Cancellation,
    work: Option<Box<dyn FnOnce() -> Result<Turn, AgentError> + Send>>,
}
impl Job {
    pub fn new<P, T, O>(task: ChildTask, mut provider: P, mut tools: T, mut observer: O) -> Self
    where
        P: Provider + Send + 'static,
        T: Tools + Send + 'static,
        O: Observer + Send + 'static,
    {
        Self {
            scope: task.scope,
            id: task.id.clone(),
            owner: task.owner.clone(),
            cancellation: task.cancellation(),
            work: Some(Box::new(move || {
                task.run(&mut provider, &mut tools, &mut observer).result
            })),
        }
    }
    fn run(mut self) -> TaskResult {
        let result = if self.cancellation.is_cancelled() {
            Err(AgentError::Cancelled)
        } else {
            catch_unwind(AssertUnwindSafe(self.work.take().expect("single-use job")))
                .unwrap_or(Err(AgentError::State("child runtime panicked")))
        };
        TaskResult {
            id: self.id.clone(),
            owner: self.owner.clone(),
            result,
        }
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

#[derive(Clone)]
pub struct TeamControl {
    owner: String,
    cancellation: Cancellation,
    tasks: Arc<Mutex<BTreeMap<String, Cancellation>>>,
}
impl TeamControl {
    fn check_owner(&self, requester: &str) -> Result<(), AgentError> {
        if requester != self.owner {
            return Err(AgentError::Configuration("team owner mismatch"));
        }
        Ok(())
    }
    pub fn cancel(&self, requester: &str) -> Result<(), AgentError> {
        self.check_owner(requester)?;
        self.cancellation.cancel();
        Ok(())
    }
    pub fn cancel_task(&self, requester: &str, id: &str) -> Result<(), AgentError> {
        self.check_owner(requester)?;
        self.tasks
            .lock()
            .map_err(|_| AgentError::State("team registry lock poisoned"))?
            .get(id)
            .ok_or(AgentError::Configuration("unknown team task"))?
            .cancel();
        Ok(())
    }
}

/// Bounded, joined native threads. Input order determines result order, not finish order.
/// Each team has a separate cancellation subtree under its parent's context.
pub struct Team {
    delegation: Delegation,
    control: TeamControl,
    concurrency: usize,
}
impl Team {
    pub fn new(
        owner: &str,
        parent: &TurnContext,
        remaining: Limits,
        tools: &impl Tools,
        concurrency: usize,
    ) -> Result<Self, AgentError> {
        if !(1..=16).contains(&concurrency) {
            return Err(AgentError::Configuration("team concurrency must be 1..16"));
        }
        let delegation = Delegation::new(owner, parent, remaining, tools)?;
        let control = TeamControl {
            owner: owner.into(),
            cancellation: delegation.cancellation(),
            tasks: Arc::new(Mutex::new(BTreeMap::new())),
        };
        Ok(Self {
            delegation,
            control,
            concurrency,
        })
    }
    pub fn control(&self) -> TeamControl {
        self.control.clone()
    }
    pub fn prepare(&self, spec: TaskSpec) -> Result<ChildTask, AgentError> {
        let task = self.delegation.prepare(spec)?;
        self.control
            .tasks
            .lock()
            .map_err(|_| AgentError::State("team registry lock poisoned"))?
            .insert(task.id.clone(), task.cancellation());
        Ok(task)
    }
    pub fn run(self, jobs: Vec<Job>) -> Result<Vec<TaskResult>, AgentError> {
        let mut ids = BTreeSet::new();
        if jobs.len() > 64
            || jobs
                .iter()
                .any(|j| j.scope != self.delegation.id || !ids.insert(j.id.clone()))
        {
            return Err(AgentError::Configuration(
                "job belongs to another team or duplicates an ID",
            ));
        }
        let count = jobs.len();
        let queue = Mutex::new(jobs.into_iter().enumerate().collect::<VecDeque<_>>());
        let results = Mutex::new(
            (0..count)
                .map(|_| None)
                .collect::<Vec<Option<TaskResult>>>(),
        );
        thread::scope(|scope| -> Result<(), AgentError> {
            // This guard runs before scoped threads are implicitly joined on unwind.
            let _cancel = CancelOnDrop(self.control.cancellation.clone());
            let mut handles = vec![];
            let mut failure = false;
            for index in 0..self.concurrency.min(count) {
                let queue = &queue;
                let results = &results;
                match thread::Builder::new()
                    .name(format!("pal-worker-{index}"))
                    .spawn_scoped(scope, move || {
                        loop {
                            let job = queue.lock().unwrap_or_else(|p| p.into_inner()).pop_front();
                            let Some((index, job)) = job else { break };
                            let result = job.run();
                            results.lock().unwrap_or_else(|p| p.into_inner())[index] = Some(result);
                        }
                    }) {
                    Ok(handle) => handles.push(handle),
                    Err(_) => {
                        self.control.cancellation.cancel();
                        failure = true;
                        break;
                    }
                }
            }
            for handle in handles {
                if handle.join().is_err() {
                    self.control.cancellation.cancel();
                    failure = true;
                }
            }
            if failure {
                Err(AgentError::State(
                    "team worker failed; all started workers joined",
                ))
            } else {
                Ok(())
            }
        })?;
        results
            .into_inner()
            .map_err(|_| AgentError::State("team results lock poisoned"))?
            .into_iter()
            .map(|r| r.ok_or(AgentError::State("team result missing")))
            .collect()
    }
}
impl Drop for Team {
    fn drop(&mut self) {
        self.control.cancellation.cancel();
    }
}
struct CancelOnDrop(Cancellation);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
