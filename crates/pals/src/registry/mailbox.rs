//! Durable local delivery. A lease timeout never authorizes replay of started work.
use super::{Record, Registry, TaskStatus, find_task, find_task_mut, require_active};
use crate::valid_id;
use namzu_core::AgentError;
use namzu_session::{SessionStatus, new_id, now_ms};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, time::Duration};
mod worker;
pub use worker::{
    BatchBudget, BatchResult, DeliveryResult, HostRuntime, MailboxHost, MailboxRuntime,
};

pub trait MailboxClock {
    fn now_ms(&self) -> u64;
}
#[derive(Clone, Copy, Default)]
pub struct SystemClock;
impl MailboxClock for SystemClock {
    fn now_ms(&self) -> u64 {
        now_ms()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MailSender {
    Operator,
    Pal { id: String },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MailMessage {
    /// Stable producer ID, unique for the lifetime of this recipient's inbox.
    pub id: String,
    pub sender: MailSender,
    pub body: String,
    pub in_reply_to: Option<String>,
}
impl MailMessage {
    pub fn operator(id: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            sender: MailSender::Operator,
            body: body.into(),
            in_reply_to: None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum DeliveryState {
    Ready,
    Leased { worker: String, until_ms: u64 },
    Acknowledged { worker: String, outcome: TaskStatus },
    NeedsInspection { worker: String },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub message: MailMessage,
    pub task_id: String,
    pub created_ms: u64,
    pub generation: u64,
    pub delivery: DeliveryState,
}
#[derive(Clone, Debug)]
pub struct SendReceipt {
    pub task_id: String,
    pub inserted: bool,
}

/// Host capability, never serialized or placed in a model conversation.
#[derive(Clone, Debug)]
pub struct Lease {
    owner: String,
    pal_id: String,
    message_id: String,
    task_id: String,
    worker: String,
    generation: u64,
}
impl Lease {
    pub fn message_id(&self) -> &str {
        &self.message_id
    }
    pub fn task_id(&self) -> &str {
        &self.task_id
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
}
pub struct Mailbox<'a, C = SystemClock> {
    registry: &'a Registry,
    owner: String,
    pal_id: String,
    clock: C,
}
impl Registry {
    pub fn mailbox(&self, owner: &str, pal_id: &str) -> Result<Mailbox<'_>, AgentError> {
        self.load(owner, pal_id)?;
        Ok(Mailbox {
            registry: self,
            owner: owner.into(),
            pal_id: pal_id.into(),
            clock: SystemClock,
        })
    }
}
impl<'a, C> Mailbox<'a, C> {
    /// Host-selected clock, useful for deterministic restart/expiry fixtures.
    pub fn with_clock<N: MailboxClock>(self, clock: N) -> Mailbox<'a, N> {
        Mailbox {
            registry: self.registry,
            owner: self.owner,
            pal_id: self.pal_id,
            clock,
        }
    }
}
impl<C: MailboxClock> Mailbox<'_, C> {
    pub fn messages(&self) -> Result<Vec<Envelope>, AgentError> {
        Ok(self.registry.load(&self.owner, &self.pal_id)?.mailbox)
    }
    pub fn send(&self, message: MailMessage) -> Result<SendReceipt, AgentError> {
        validate_message(&message)?;
        let _lock = self.registry.store.lock("registry")?;
        let mut record = self.registry.load(&self.owner, &self.pal_id)?;
        if let Some(existing) = record.mailbox.iter().find(|e| e.message.id == message.id) {
            if existing.message != message {
                return Err(AgentError::State(
                    "message ID already has a different payload or sender",
                ));
            }
            return Ok(SendReceipt {
                task_id: existing.task_id.clone(),
                inserted: false,
            });
        }
        require_active(record.revisions.last().expect("validated revisions"))?;
        if let MailSender::Pal { id } = &message.sender {
            let source = self.registry.load(&self.owner, id)?;
            require_active(source.revisions.last().expect("validated revisions"))?;
        }
        if record.mailbox.len() >= 128 {
            return Err(AgentError::State("mailbox message limit"));
        }
        let task_id = format!("m-{}", new_id());
        self.registry
            .append_task(&self.pal_id, &mut record, &task_id, &message.body, None)?;
        // Task and message reference become visible in the same atomic manifest write.
        record.mailbox.push(Envelope {
            message,
            task_id: task_id.clone(),
            created_ms: self.clock.now_ms(),
            generation: 0,
            delivery: DeliveryState::Ready,
        });
        self.registry.save(&self.pal_id, &record)?;
        Ok(SendReceipt {
            task_id,
            inserted: true,
        })
    }
    /// Reconcile only expired claims while no Pal execution owns the process lock.
    pub fn recover(&self) -> Result<(), AgentError> {
        let _run = self.registry.store.lock(&format!("{}-run", self.pal_id))?;
        let _lock = self.registry.store.lock("registry")?;
        let mut record = self.registry.load(&self.owner, &self.pal_id)?;
        if self.reconcile(&mut record)? {
            self.registry.save(&self.pal_id, &record)?;
        }
        Ok(())
    }
    pub fn claim(&self, worker: &str, lease_for: Duration) -> Result<Option<Lease>, AgentError> {
        if !valid_id(worker) {
            return Err(AgentError::Configuration("invalid mailbox worker ID"));
        }
        let until_ms = deadline(self.clock.now_ms(), lease_for)?;
        // Same lock order as start_task: no expiry takeover of a live worker.
        let _run = self.registry.store.lock(&format!("{}-run", self.pal_id))?;
        let _lock = self.registry.store.lock("registry")?;
        let mut record = self.registry.load(&self.owner, &self.pal_id)?;
        if self.reconcile(&mut record)? {
            self.registry.save(&self.pal_id, &record)?;
        }
        require_active(record.revisions.last().expect("validated revisions"))?;
        if record.tasks.iter().any(|t| t.status == TaskStatus::Running)
            || record
                .mailbox
                .iter()
                .any(|e| matches!(e.delivery, DeliveryState::NeedsInspection { .. }))
        {
            return Ok(None);
        }
        // A Pal has one conversation/workspace owner, including the setup interval.
        if record
            .mailbox
            .iter()
            .any(|e| matches!(e.delivery, DeliveryState::Leased { .. }))
        {
            return Ok(None);
        }
        let Some(index) = record
            .mailbox
            .iter()
            .position(|e| e.delivery == DeliveryState::Ready)
        else {
            return Ok(None);
        };
        let task = find_task(&record, &record.mailbox[index].task_id)?;
        let pal = &record.revisions[(task.revision - 1) as usize];
        self.registry
            .load_session(&self.pal_id, task, pal)?
            .require_resumable(&pal.workspace, &pal.route)?;
        if task.status != TaskStatus::Queued {
            return Err(AgentError::State("ready message has nonqueued task"));
        }
        let envelope = &mut record.mailbox[index];
        envelope.generation = envelope
            .generation
            .checked_add(1)
            .ok_or(AgentError::State("lease generation exhausted"))?;
        envelope.delivery = DeliveryState::Leased {
            worker: worker.into(),
            until_ms,
        };
        let lease = Lease {
            owner: self.owner.clone(),
            pal_id: self.pal_id.clone(),
            message_id: envelope.message.id.clone(),
            task_id: envelope.task_id.clone(),
            worker: worker.into(),
            generation: envelope.generation,
        };
        self.registry.save(&self.pal_id, &record)?;
        Ok(Some(lease))
    }
    pub fn renew(&self, lease: &Lease, lease_for: Duration) -> Result<(), AgentError> {
        let until_ms = deadline(self.clock.now_ms(), lease_for)?;
        let _lock = self.registry.store.lock("registry")?;
        let mut record = self.registry.load(&self.owner, &self.pal_id)?;
        let index = self.validate_lease(&record, lease, true)?;
        let DeliveryState::Leased {
            until_ms: current, ..
        } = &mut record.mailbox[index].delivery
        else {
            unreachable!()
        };
        *current = (*current).max(until_ms);
        self.registry.save(&self.pal_id, &record)
    }
    /// Release only a claim with a pristine queued session; started work cannot retry.
    pub fn release(&self, lease: &Lease) -> Result<(), AgentError> {
        let _run = self.registry.store.lock(&format!("{}-run", self.pal_id))?;
        let _lock = self.registry.store.lock("registry")?;
        let mut record = self.registry.load(&self.owner, &self.pal_id)?;
        let index = self.validate_lease(&record, lease, true)?;
        let task = find_task(&record, &lease.task_id)?;
        let pal = &record.revisions[(task.revision - 1) as usize];
        let session = self.registry.load_session(&self.pal_id, task, pal)?;
        if task.status != TaskStatus::Queued {
            return Err(AgentError::State("started delivery cannot be released"));
        }
        session.require_resumable(&pal.workspace, &pal.route)?;
        record.mailbox[index].delivery = DeliveryState::Ready;
        self.registry.save(&self.pal_id, &record)
    }
    /// Acknowledge a durably terminal task; this method never runs model/tool work.
    pub fn ack(&self, lease: &Lease) -> Result<DeliveryState, AgentError> {
        let _run = self.registry.store.lock(&format!("{}-run", self.pal_id))?;
        let _lock = self.registry.store.lock("registry")?;
        let mut record = self.registry.load(&self.owner, &self.pal_id)?;
        let index = self.validate_lease(&record, lease, false)?;
        if matches!(
            record.mailbox[index].delivery,
            DeliveryState::Acknowledged { .. }
        ) {
            return Ok(record.mailbox[index].delivery.clone());
        }
        if !matches!(record.mailbox[index].delivery, DeliveryState::Leased { .. }) {
            return Err(AgentError::State("delivery requires inspection"));
        }
        let task = find_task(&record, &lease.task_id)?;
        if !terminal(task.status) {
            return Err(AgentError::State("only terminal tasks can be acknowledged"));
        }
        let delivery = self.terminal_delivery(&record, index, &lease.worker)?;
        record.mailbox[index].delivery = delivery.clone();
        self.registry.save(&self.pal_id, &record)?;
        Ok(delivery)
    }
    /// Explicit host acknowledgement after inspecting effects; no replay or rollback.
    pub fn resolve_inspection(&self, message_id: &str) -> Result<(), AgentError> {
        let _run = self.registry.store.lock(&format!("{}-run", self.pal_id))?;
        let _lock = self.registry.store.lock("registry")?;
        let mut record = self.registry.load(&self.owner, &self.pal_id)?;
        let index = record
            .mailbox
            .iter()
            .position(|e| e.message.id == message_id)
            .ok_or(AgentError::State("message not found"))?;
        let DeliveryState::NeedsInspection { worker } = &record.mailbox[index].delivery else {
            return Err(AgentError::State("message needs no inspection"));
        };
        let worker = worker.clone();
        let task_id = record.mailbox[index].task_id.clone();
        let task = find_task(&record, &task_id)?;
        let pal = &record.revisions[(task.revision - 1) as usize];
        let mut session = self.registry.load_session(&self.pal_id, task, pal)?;
        session.status = SessionStatus::Failed;
        session.updated_ms = now_ms();
        session.save(&self.registry.store.open_child(&self.pal_id)?)?;
        let task = find_task_mut(&mut record, &task_id)?;
        task.status = TaskStatus::Interrupted;
        task.updated_ms = now_ms();
        record.mailbox[index].delivery = DeliveryState::Acknowledged {
            worker,
            outcome: TaskStatus::Interrupted,
        };
        self.registry.save(&self.pal_id, &record)
    }
    fn validate_lease(
        &self,
        record: &Record,
        lease: &Lease,
        live: bool,
    ) -> Result<usize, AgentError> {
        if lease.owner != self.owner || lease.pal_id != self.pal_id {
            return Err(AgentError::State("lease recipient mismatch"));
        }
        let index = record
            .mailbox
            .iter()
            .position(|e| e.message.id == lease.message_id)
            .ok_or(AgentError::State("lease message missing"))?;
        let envelope = &record.mailbox[index];
        if envelope.task_id != lease.task_id || envelope.generation != lease.generation {
            return Err(AgentError::State("stale delivery generation"));
        }
        let worker = match &envelope.delivery {
            DeliveryState::Leased { worker, until_ms }
                if !live || self.clock.now_ms() < *until_ms =>
            {
                worker
            }
            DeliveryState::Acknowledged { worker, .. }
            | DeliveryState::NeedsInspection { worker }
                if !live =>
            {
                worker
            }
            _ => {
                return Err(AgentError::State(
                    "delivery lease is expired or unavailable",
                ));
            }
        };
        if worker != &lease.worker {
            return Err(AgentError::State("lease worker mismatch"));
        }
        Ok(index)
    }
    fn reconcile(&self, record: &mut Record) -> Result<bool, AgentError> {
        let mut changed = false;
        for index in 0..record.mailbox.len() {
            let DeliveryState::Leased { worker, until_ms } = &record.mailbox[index].delivery else {
                continue;
            };
            if self.clock.now_ms() < *until_ms {
                continue;
            }
            let worker = worker.clone();
            let task = find_task(record, &record.mailbox[index].task_id)?;
            let pal = &record.revisions[(task.revision - 1) as usize];
            let session = self.registry.load_session(&self.pal_id, task, pal)?;
            record.mailbox[index].delivery = if terminal(task.status) {
                self.terminal_delivery(record, index, &worker)?
            } else if task.status == TaskStatus::Queued
                && session.status == SessionStatus::Complete
                && session.pending_call.is_none()
            {
                DeliveryState::Ready
            } else {
                DeliveryState::NeedsInspection { worker }
            };
            changed = true;
        }
        Ok(changed)
    }
    fn terminal_delivery(
        &self,
        record: &Record,
        index: usize,
        worker: &str,
    ) -> Result<DeliveryState, AgentError> {
        let task = find_task(record, &record.mailbox[index].task_id)?;
        let pal = &record.revisions[(task.revision - 1) as usize];
        let session = self.registry.load_session(&self.pal_id, task, pal)?;
        let consistent = if task.status == TaskStatus::Complete {
            session.status == SessionStatus::Complete
        } else {
            session.status == SessionStatus::Failed
        };
        Ok(if consistent && session.pending_call.is_none() {
            DeliveryState::Acknowledged {
                worker: worker.into(),
                outcome: task.status,
            }
        } else {
            DeliveryState::NeedsInspection {
                worker: worker.into(),
            }
        })
    }
}
fn terminal(status: TaskStatus) -> bool {
    !matches!(status, TaskStatus::Queued | TaskStatus::Running)
}
fn deadline(now: u64, duration: Duration) -> Result<u64, AgentError> {
    if duration < Duration::from_millis(1) || duration > Duration::from_secs(86400) {
        return Err(AgentError::Configuration(
            "delivery lease must be 1 ms..86400 seconds",
        ));
    }
    now.checked_add(duration.as_millis() as u64)
        .ok_or(AgentError::Configuration("delivery lease clock overflow"))
}
fn validate_message(message: &MailMessage) -> Result<(), AgentError> {
    if !valid_id(&message.id)
        || message.body.is_empty()
        || message.body.len() > 16 * 1024
        || message.in_reply_to.as_ref().is_some_and(|id| !valid_id(id))
        || matches!(&message.sender,MailSender::Pal{id} if !valid_id(id))
    {
        return Err(AgentError::Configuration("invalid mailbox message"));
    }
    Ok(())
}
pub(super) fn validate_record(record: &Record) -> Result<(), AgentError> {
    let mut messages = BTreeSet::new();
    let mut tasks = BTreeSet::new();
    if record.mailbox.len() > 128 {
        return Err(AgentError::State("mailbox record limit"));
    }
    for envelope in &record.mailbox {
        validate_message(&envelope.message)?;
        let task = find_task(record, &envelope.task_id)?;
        if !messages.insert(&envelope.message.id)
            || !tasks.insert(&envelope.task_id)
            || task.follows.is_some()
        {
            return Err(AgentError::State("invalid mailbox task binding"));
        }
        match &envelope.delivery {
            DeliveryState::Ready => {}
            DeliveryState::Leased { worker, until_ms } => {
                if !valid_id(worker) || *until_ms == 0 || envelope.generation == 0 {
                    return Err(AgentError::State("invalid delivery lease"));
                }
            }
            DeliveryState::Acknowledged { worker, outcome } => {
                if !valid_id(worker)
                    || envelope.generation == 0
                    || !terminal(*outcome)
                    || *outcome != task.status
                {
                    return Err(AgentError::State("invalid delivery acknowledgement"));
                }
            }
            DeliveryState::NeedsInspection { worker } => {
                if !valid_id(worker) || envelope.generation == 0 {
                    return Err(AgentError::State("invalid inspection state"));
                }
            }
        }
    }
    Ok(())
}
