use crate::{RestrictedTools, valid_id};
use namzu_core::{
    AgentError, Event, Limits, Message, Observer, Provider, Stats, ToolCall, Tools, Turn,
    TurnContext, run_history_in_context,
};
use namzu_session::{Journal, PrivateStore, Session, SessionStatus, StoreLock, new_id, now_ms};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
mod mailbox;
pub use mailbox::{
    BatchBudget, BatchResult, DeliveryResult, DeliveryState, Envelope, HostRuntime, Lease,
    MailMessage, MailSender, Mailbox, MailboxClock, MailboxHost, MailboxRuntime, SendReceipt,
    SystemClock,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PalStatus {
    Active,
    Paused,
    Archived,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PalDefinition {
    pub version: u32,
    pub id: String,
    pub owner: String,
    pub name: String,
    pub purpose: String,
    /// Host output directory, not an isolated Pal computer.
    pub workspace: PathBuf,
    pub route: String,
    pub tools: BTreeSet<String>,
    pub status: PalStatus,
    pub revision: u64,
    pub created_ms: u64,
    pub updated_ms: u64,
}
pub struct PalCreate {
    pub owner: String,
    pub name: String,
    pub purpose: String,
    pub route: String,
    pub tools: Option<BTreeSet<String>>,
}
#[derive(Default)]
pub struct PalUpdate {
    pub name: Option<String>,
    pub purpose: Option<String>,
    pub route: Option<String>,
    pub tools: Option<BTreeSet<String>>,
    pub status: Option<PalStatus>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Running,
    Complete,
    Failed,
    Cancelled,
    Interrupted,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRecord {
    pub id: String,
    pub revision: u64,
    pub session_id: String,
    pub status: TaskStatus,
    pub follows: Option<String>,
    pub created_ms: u64,
    pub updated_ms: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    revisions: Vec<PalDefinition>,
    tasks: Vec<TaskRecord>,
    #[serde(default)]
    mailbox: Vec<Envelope>,
}

/// Two explicit private roots: host metadata cannot be exposed through workspace tools.
pub struct Registry {
    store: PrivateStore,
    workspaces: PrivateStore,
    workspace_root: PathBuf,
}
impl Registry {
    pub fn open(
        state_root: impl AsRef<Path>,
        workspace_root: impl AsRef<Path>,
    ) -> Result<Self, AgentError> {
        let store = PrivateStore::open(state_root.as_ref())?;
        let workspaces = PrivateStore::open(workspace_root.as_ref())?;
        let state = state_root
            .as_ref()
            .canonicalize()
            .map_err(|_| AgentError::State("invalid registry root"))?;
        let workspace_root = workspace_root
            .as_ref()
            .canonicalize()
            .map_err(|_| AgentError::State("invalid workspace root"))?;
        if state.starts_with(&workspace_root) || workspace_root.starts_with(&state) {
            return Err(AgentError::Configuration(
                "registry and workspace roots must not overlap",
            ));
        }
        Ok(Self {
            store,
            workspaces,
            workspace_root,
        })
    }
    fn load(&self, owner: &str, id: &str) -> Result<Record, AgentError> {
        if !valid_id(owner) || !valid_id(id) {
            return Err(AgentError::State("invalid Pal identity"));
        }
        let record: Record = self
            .store
            .read(&format!("{id}.json"))?
            .ok_or(AgentError::State("Pal not found"))?;
        if record.version != 1
            || record.revisions.is_empty()
            || record.revisions.len() > 64
            || record.tasks.len() > 128
        {
            return Err(AgentError::State("invalid Pal record"));
        }
        for (i, pal) in record.revisions.iter().enumerate() {
            validate_definition(pal)?;
            if pal.id != id
                || pal.owner != owner
                || pal.revision != (i + 1) as u64
                || pal.workspace != self.workspace_root.join(id)
            {
                return Err(AgentError::State(
                    "Pal ownership, workspace or revision mismatch",
                ));
            }
        }
        let mut ids = BTreeSet::new();
        let mut sessions = BTreeSet::new();
        for task in &record.tasks {
            if !valid_id(&task.id)
                || !valid_id(&task.session_id)
                || !ids.insert(&task.id)
                || !sessions.insert(&task.session_id)
                || task.revision == 0
                || task.revision > record.revisions.len() as u64
            {
                return Err(AgentError::State("invalid Pal task record"));
            }
            if let Some(source) = &task.follows {
                let parent = record
                    .tasks
                    .iter()
                    .find(|t| &t.id == source)
                    .ok_or(AgentError::State("missing followup source"))?;
                if parent.id == task.id
                    || parent.status != TaskStatus::Complete
                    || parent.revision != task.revision
                {
                    return Err(AgentError::State("invalid followup source"));
                }
            }
        }
        // Missing/replaced directories fail closed; never recreate them on load.
        mailbox::validate_record(&record)?;
        self.workspaces.open_child(id)?;
        self.store.open_child(id)?;
        Ok(record)
    }
    fn save(&self, id: &str, record: &Record) -> Result<(), AgentError> {
        self.store.write(&format!("{id}.json"), record)
    }
    pub fn create(&self, input: PalCreate) -> Result<PalDefinition, AgentError> {
        let _lock = self.store.lock("registry")?;
        let id = format!("p-{}", new_id());
        let now = now_ms();
        let pal = PalDefinition {
            version: 1,
            id: id.clone(),
            owner: input.owner,
            name: input.name,
            purpose: input.purpose,
            workspace: self.workspace_root.join(&id),
            route: input.route,
            tools: input
                .tools
                .unwrap_or_else(|| super::READ_ONLY.iter().map(|s| (*s).into()).collect()),
            status: PalStatus::Active,
            revision: 1,
            created_ms: now,
            updated_ms: now,
        };
        validate_definition(&pal)?;
        self.workspaces.child(&id)?;
        self.store.child(&id)?;
        self.store.write_new(
            &format!("{id}.json"),
            &Record {
                version: 1,
                revisions: vec![pal.clone()],
                tasks: vec![],
                mailbox: vec![],
            },
        )?;
        Ok(pal)
    }
    pub fn get(&self, owner: &str, id: &str) -> Result<PalDefinition, AgentError> {
        Ok(self
            .load(owner, id)?
            .revisions
            .last()
            .expect("validated revisions")
            .clone())
    }
    pub fn revision(
        &self,
        owner: &str,
        id: &str,
        revision: u64,
    ) -> Result<PalDefinition, AgentError> {
        let record = self.load(owner, id)?;
        record
            .revisions
            .iter()
            .find(|r| r.revision == revision)
            .cloned()
            .ok_or(AgentError::State("Pal revision not found"))
    }
    pub fn list(&self, owner: &str) -> Result<Vec<PalDefinition>, AgentError> {
        if !valid_id(owner) {
            return Err(AgentError::State("invalid owner"));
        }
        let mut result = vec![];
        for name in self.store.names()? {
            if let Some(id) = name.strip_suffix(".json").filter(|id| id.starts_with("p-")) {
                // Inspect owner without ever returning another owner's definition.
                let record: Record = self
                    .store
                    .read(&name)?
                    .ok_or(AgentError::State("Pal disappeared"))?;
                if record.revisions.last().is_some_and(|r| r.owner == owner) {
                    result.push(self.get(owner, id)?);
                }
            }
        }
        Ok(result)
    }
    pub fn update(
        &self,
        owner: &str,
        id: &str,
        expected_revision: u64,
        changes: PalUpdate,
    ) -> Result<PalDefinition, AgentError> {
        let _lock = self.store.lock("registry")?;
        let mut record = self.load(owner, id)?;
        let mut pal = record
            .revisions
            .last()
            .expect("validated revisions")
            .clone();
        if pal.revision != expected_revision
            || record.revisions.len() >= 64
            || pal.status == PalStatus::Archived
        {
            return Err(AgentError::State(
                "Pal revision conflict, archived or revision limit",
            ));
        }
        if let Some(v) = changes.name {
            pal.name = v;
        }
        if let Some(v) = changes.purpose {
            pal.purpose = v;
        }
        if let Some(v) = changes.route {
            pal.route = v;
        }
        if let Some(v) = changes.tools {
            pal.tools = v;
        }
        if let Some(v) = changes.status {
            pal.status = v;
        }
        pal.revision += 1;
        pal.updated_ms = now_ms();
        validate_definition(&pal)?;
        record.revisions.push(pal.clone());
        self.save(id, &record)?;
        Ok(pal)
    }
    pub fn tasks(&self, owner: &str, id: &str) -> Result<Vec<TaskRecord>, AgentError> {
        Ok(self.load(owner, id)?.tasks)
    }
    pub fn queue_task(
        &self,
        owner: &str,
        id: &str,
        task_id: &str,
        prompt: &str,
    ) -> Result<TaskRecord, AgentError> {
        self.queue_inner(owner, id, task_id, prompt, None)
    }
    /// Explicit continuation forks a completed session into a new task/session.
    /// The original result stays immutable; failed/uncertain sessions cannot be used.
    pub fn queue_followup(
        &self,
        owner: &str,
        id: &str,
        task_id: &str,
        source_task: &str,
        prompt: &str,
    ) -> Result<TaskRecord, AgentError> {
        self.queue_inner(owner, id, task_id, prompt, Some(source_task))
    }
    fn queue_inner(
        &self,
        owner: &str,
        id: &str,
        task_id: &str,
        prompt: &str,
        source: Option<&str>,
    ) -> Result<TaskRecord, AgentError> {
        let _lock = self.store.lock("registry")?;
        let mut record = self.load(owner, id)?;
        let task = self.append_task(id, &mut record, task_id, prompt, source)?;
        self.save(id, &record)?;
        Ok(task)
    }
    fn append_task(
        &self,
        id: &str,
        record: &mut Record,
        task_id: &str,
        prompt: &str,
        source: Option<&str>,
    ) -> Result<TaskRecord, AgentError> {
        if !valid_id(task_id) || prompt.is_empty() || prompt.len() > 16 * 1024 {
            return Err(AgentError::Configuration("invalid persistent task"));
        }
        require_active(record.revisions.last().expect("validated revisions"))?;
        if record.tasks.len() >= 128 || record.tasks.iter().any(|t| t.id == task_id) {
            return Err(AgentError::State("duplicate task or task limit"));
        }
        let (pal, messages) = if let Some(source) = source {
            let task = find_task(record, source)?;
            if task.status != TaskStatus::Complete {
                return Err(AgentError::State("followup requires a completed task"));
            }
            let pal = &record.revisions[(task.revision - 1) as usize];
            let session = self.load_session(id, task, pal)?;
            session.require_resumable(&pal.workspace, &pal.route)?;
            (pal, session.messages)
        } else {
            (
                record.revisions.last().expect("validated revisions"),
                vec![],
            )
        };
        let mut session = Session::new(new_id(), pal.workspace.clone(), pal.route.clone())?;
        session.messages = messages;
        session.messages.push(Message::User {
            content: prompt.into(),
        });
        let state = self.store.open_child(id)?;
        // Session first: a crash can leave an unreferenced file, never an executable task.
        state.write_new(&format!("{}.json", session.id), &session)?;
        let now = now_ms();
        let task = TaskRecord {
            id: task_id.into(),
            revision: pal.revision,
            session_id: session.id,
            status: TaskStatus::Queued,
            follows: source.map(String::from),
            created_ms: now,
            updated_ms: now,
        };
        record.tasks.push(task.clone());
        Ok(task)
    }
    pub fn session(&self, owner: &str, id: &str, task_id: &str) -> Result<Session, AgentError> {
        let record = self.load(owner, id)?;
        let task = find_task(&record, task_id)?;
        self.load_session(id, task, &record.revisions[(task.revision - 1) as usize])
    }
    fn load_session(
        &self,
        id: &str,
        task: &TaskRecord,
        pal: &PalDefinition,
    ) -> Result<Session, AgentError> {
        let session = Session::load(&self.store.open_child(id)?, &task.session_id)?;
        if session.cwd != pal.workspace || session.route != pal.route {
            return Err(AgentError::State("Pal session binding mismatch"));
        }
        Ok(session)
    }
    pub fn start_task(
        &self,
        owner: &str,
        id: &str,
        task_id: &str,
    ) -> Result<PalRun<'_>, AgentError> {
        self.start_task_checked(owner, id, task_id, |record| {
            if record.mailbox.iter().any(|e| e.task_id == task_id) {
                return Err(AgentError::State(
                    "mailbox task requires its delivery lease",
                ));
            }
            Ok(())
        })
    }
    fn start_task_checked(
        &self,
        owner: &str,
        id: &str,
        task_id: &str,
        admit: impl FnOnce(&Record) -> Result<(), AgentError>,
    ) -> Result<PalRun<'_>, AgentError> {
        if !valid_id(id) {
            return Err(AgentError::State("invalid Pal ID"));
        }
        let run_lock = self.store.lock(&format!("{id}-run"))?;
        let _lock = self.store.lock("registry")?;
        let mut record = self.load(owner, id)?;
        admit(&record)?;
        require_active(record.revisions.last().expect("validated revisions"))?;
        if record.tasks.iter().any(|t| t.status == TaskStatus::Running) {
            return Err(AgentError::State(
                "uncertain running task; inspect and acknowledge interruption",
            ));
        }
        let task = find_task(&record, task_id)?.clone();
        if task.status != TaskStatus::Queued {
            return Err(AgentError::State(
                "task is not queued; automatic replay refused",
            ));
        }
        let pal = record.revisions[(task.revision - 1) as usize].clone();
        let mut session = self.load_session(id, &task, &pal)?;
        session.require_resumable(&pal.workspace, &pal.route)?;
        session.status = SessionStatus::Running;
        session.updated_ms = now_ms();
        session.save(&self.store.open_child(id)?)?;
        let entry = find_task_mut(&mut record, task_id)?;
        entry.status = TaskStatus::Running;
        entry.updated_ms = now_ms();
        self.save(id, &record)?;
        Ok(PalRun {
            registry: self,
            pal,
            task_id: task_id.into(),
            session,
            _run_lock: run_lock,
            finished: false,
        })
    }
    /// Explicit inspection acknowledgement; closes uncertain work without replaying it.
    pub fn acknowledge_interruption(
        &self,
        owner: &str,
        id: &str,
        task_id: &str,
    ) -> Result<(), AgentError> {
        if !valid_id(id) {
            return Err(AgentError::State("invalid Pal ID"));
        }
        let _run = self.store.lock(&format!("{id}-run"))?;
        let _lock = self.store.lock("registry")?;
        let mut record = self.load(owner, id)?;
        if record.mailbox.iter().any(|e| e.task_id == task_id) {
            return Err(AgentError::State(
                "mailbox interruption requires mailbox inspection resolution",
            ));
        }
        let task = find_task(&record, task_id)?.clone();
        let pal = &record.revisions[(task.revision - 1) as usize];
        let mut session = self.load_session(id, &task, pal)?;
        if task.status != TaskStatus::Running
            && !(task.status == TaskStatus::Queued && session.status != SessionStatus::Complete)
        {
            return Err(AgentError::State("task has no uncertain running state"));
        }
        session.status = SessionStatus::Failed;
        session.updated_ms = now_ms();
        session.save(&self.store.open_child(id)?)?;
        let task = find_task_mut(&mut record, task_id)?;
        task.status = TaskStatus::Interrupted;
        task.updated_ms = now_ms();
        self.save(id, &record)
    }
}
fn require_active(pal: &PalDefinition) -> Result<(), AgentError> {
    if pal.status != PalStatus::Active {
        return Err(AgentError::State("Pal is paused or archived"));
    }
    Ok(())
}
fn validate_definition(pal: &PalDefinition) -> Result<(), AgentError> {
    if pal.version != 1
        || !valid_id(&pal.id)
        || !valid_id(&pal.owner)
        || pal.name.trim().is_empty()
        || pal.name.len() > 128
        || pal.name.chars().any(char::is_control)
        || pal.purpose.len() > 4096
        || pal.route.is_empty()
        || pal.route.len() > 1024
        || pal.tools.len() > 128
        || pal
            .tools
            .iter()
            .any(|s| s.is_empty() || s.len() > 128 || s.chars().any(char::is_control))
    {
        return Err(AgentError::Configuration("invalid Pal definition"));
    }
    Ok(())
}
fn find_task<'a>(record: &'a Record, id: &str) -> Result<&'a TaskRecord, AgentError> {
    record
        .tasks
        .iter()
        .find(|t| t.id == id)
        .ok_or(AgentError::State("Pal task not found"))
}
fn find_task_mut<'a>(record: &'a mut Record, id: &str) -> Result<&'a mut TaskRecord, AgentError> {
    record
        .tasks
        .iter_mut()
        .find(|t| t.id == id)
        .ok_or(AgentError::State("Pal task not found"))
}

/// Exclusive conversation/workspace lease, held through all model/tool/checkpoint work.
pub struct PalRun<'a> {
    registry: &'a Registry,
    pal: PalDefinition,
    task_id: String,
    session: Session,
    _run_lock: StoreLock,
    finished: bool,
}
impl PalRun<'_> {
    pub fn definition(&self) -> &PalDefinition {
        &self.pal
    }
    pub fn session_id(&self) -> &str {
        &self.session.id
    }
    fn assert_active(&self) -> Result<(), AgentError> {
        let record = self.registry.load(&self.pal.owner, &self.pal.id)?;
        require_active(record.revisions.last().expect("validated revisions"))?;
        let task = find_task(&record, &self.task_id)?;
        if task.status != TaskStatus::Running
            || task.session_id != self.session.id
            || task.revision != self.pal.revision
        {
            return Err(AgentError::State("Pal execution ownership changed"));
        }
        Ok(())
    }
    pub fn execute(
        mut self,
        provider: &mut impl Provider,
        tools: &mut impl Tools,
        parent: &TurnContext,
        limits: Limits,
        host: &mut impl Observer,
    ) -> Result<Turn, AgentError> {
        let messages = self.session.messages.clone();
        let mut tools = RestrictedTools {
            inner: tools,
            admitted: self.pal.tools.clone(),
        };
        let result = run_history_in_context(
            provider,
            &mut tools,
            messages,
            limits,
            parent,
            &mut RunObserver {
                run: &mut self,
                host,
            },
        );
        let status = match &result {
            Ok(_) => TaskStatus::Complete,
            Err(AgentError::Cancelled) => TaskStatus::Cancelled,
            Err(_) => TaskStatus::Failed,
        };
        self.finish(status)?;
        result
    }
    fn finish(&mut self, status: TaskStatus) -> Result<(), AgentError> {
        self.session.status = if status == TaskStatus::Complete {
            SessionStatus::Complete
        } else {
            SessionStatus::Failed
        };
        self.session.updated_ms = now_ms();
        self.session
            .save(&self.registry.store.open_child(&self.pal.id)?)?;
        let _lock = self.registry.store.lock("registry")?;
        let mut record = self.registry.load(&self.pal.owner, &self.pal.id)?;
        let task = find_task_mut(&mut record, &self.task_id)?;
        if task.status != TaskStatus::Running || task.session_id != self.session.id {
            return Err(AgentError::State("Pal finish ownership mismatch"));
        }
        task.status = status;
        task.updated_ms = now_ms();
        self.registry.save(&self.pal.id, &record)?;
        self.finished = true;
        Ok(())
    }
}
impl Drop for PalRun<'_> {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.finish(TaskStatus::Interrupted);
        }
    }
}
struct RunObserver<'a, 'r, O> {
    run: &'a mut PalRun<'r>,
    host: &'a mut O,
}
impl<O: Observer> Observer for RunObserver<'_, '_, O> {
    fn approve(&mut self, call: &ToolCall, context: &TurnContext) -> Result<bool, AgentError> {
        self.run.assert_active()?;
        let approved = self.host.approve(call, context)?;
        self.run.assert_active()?;
        Ok(approved)
    }
    fn event(&mut self, event: Event) -> Result<(), AgentError> {
        let before_effect = matches!(
            event,
            Event::ProviderStarted { .. } | Event::ToolStarted { .. }
        );
        self.host.event(event)?;
        if before_effect {
            self.run.assert_active()?;
        }
        Ok(())
    }
    fn checkpoint(
        &mut self,
        messages: &[Message],
        stats: &Stats,
        pending: Option<&ToolCall>,
    ) -> Result<(), AgentError> {
        self.run.assert_active()?;
        Journal {
            store: &self.run.registry.store.open_child(&self.run.pal.id)?,
            session: &mut self.run.session,
        }
        .checkpoint(messages, stats, pending)?;
        self.host.checkpoint(messages, stats, pending)?;
        self.run.assert_active()
    }
}
