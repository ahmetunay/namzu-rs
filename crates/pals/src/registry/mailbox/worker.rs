use super::{DeliveryState, Lease, Mailbox, MailboxClock, TaskStatus, find_task};
use crate::{PalDefinition, PalRun};
use namzu_core::{
    AgentError, Event, Limits, Message, Observer, Provider, Stats, ToolCall, Tools, Turn,
    TurnContext,
};
use std::time::Duration;

pub struct MailboxRuntime<P, T, O> {
    pub provider: P,
    pub tools: T,
    pub observer: O,
}
pub type HostRuntime<H> = MailboxRuntime<
    <H as MailboxHost>::Provider,
    <H as MailboxHost>::Tools,
    <H as MailboxHost>::Observer,
>;
/// Trusted host binds the pinned route and workspace, without granting new tools.
pub trait MailboxHost {
    type Provider: Provider;
    type Tools: Tools;
    type Observer: Observer;
    fn open(
        &mut self,
        pal: &PalDefinition,
        context: &TurnContext,
    ) -> Result<HostRuntime<Self>, AgentError>;
}
/// Scoped runtime adapter: borrowed container leases need not become self-referential
/// owned values. Implementations must execute the run with the supplied guard.
pub trait MailboxExecutor {
    fn execute(
        &mut self,
        run: PalRun<'_>,
        context: &TurnContext,
        limits: Limits,
        guard: &mut dyn Observer,
    ) -> Result<Turn, AgentError>;
}
/// Compose host presentation with delivery fencing, before and after callbacks.
pub struct GuardedObserver<'a> {
    pub host: &'a mut dyn Observer,
    pub guard: &'a mut dyn Observer,
}
impl Observer for GuardedObserver<'_> {
    fn supports_approval(&self) -> bool {
        self.host.supports_approval()
    }
    fn approve(&mut self, call: &ToolCall, context: &TurnContext) -> Result<bool, AgentError> {
        if !self.guard.approve(call, context)? {
            return Ok(false);
        }
        let approved = self.host.approve(call, context)?;
        Ok(self.guard.approve(call, context)? && approved)
    }
    fn event(&mut self, event: Event) -> Result<(), AgentError> {
        self.guard.event(event.clone())?;
        self.host.event(event.clone())?;
        self.guard.event(event)
    }
    fn checkpoint(
        &mut self,
        messages: &[Message],
        stats: &Stats,
        pending: Option<&ToolCall>,
    ) -> Result<(), AgentError> {
        self.guard.checkpoint(messages, stats, pending)?;
        self.host.checkpoint(messages, stats, pending)?;
        self.guard.checkpoint(messages, stats, pending)
    }
}
impl<H: MailboxHost> MailboxExecutor for H {
    fn execute(
        &mut self,
        mut run: PalRun<'_>,
        context: &TurnContext,
        limits: Limits,
        guard: &mut dyn Observer,
    ) -> Result<Turn, AgentError> {
        let mut runtime = match context
            .remaining()
            .and_then(|_| self.open(run.definition(), context))
        {
            Ok(runtime) => runtime,
            Err(error) => {
                run.finish(if error == AgentError::Cancelled {
                    TaskStatus::Cancelled
                } else {
                    TaskStatus::Failed
                })?;
                return Err(error);
            }
        };
        run.execute(
            &mut runtime.provider,
            &mut runtime.tools,
            context,
            limits,
            &mut GuardedObserver {
                host: &mut runtime.observer,
                guard,
            },
        )
    }
}
pub struct DeliveryResult {
    pub message_id: String,
    pub task_id: String,
    pub delivery: DeliveryState,
    pub result: Result<Turn, AgentError>,
}
pub struct BatchResult {
    pub deliveries: Vec<DeliveryResult>,
    pub stopped: Option<AgentError>,
}
/// Reserve the worst-case aggregate budget before admitting the first delivery.
pub struct BatchBudget {
    pub max_messages: usize,
    pub per_message: Limits,
    pub total: Limits,
}
impl Default for BatchBudget {
    fn default() -> Self {
        Self {
            max_messages: 4,
            per_message: Limits {
                max_steps: 16,
                max_tool_calls: 16,
                max_text_bytes: 256 * 1024,
                ..Limits::default()
            },
            total: Limits::default(),
        }
    }
}
impl BatchBudget {
    fn validate(&self) -> Result<(), AgentError> {
        validate_limits(&self.per_message)?;
        validate_limits(&self.total)?;
        if !(1..=64).contains(&self.max_messages)
            || self
                .per_message
                .max_steps
                .checked_mul(self.max_messages)
                .is_none_or(|v| v > self.total.max_steps)
            || self
                .per_message
                .max_tool_calls
                .checked_mul(self.max_messages)
                .is_none_or(|v| v > self.total.max_tool_calls)
            || self
                .per_message
                .max_text_bytes
                .checked_mul(self.max_messages)
                .is_none_or(|v| v > self.total.max_text_bytes)
        {
            return Err(AgentError::Configuration(
                "mailbox batch exceeds aggregate delegation budget",
            ));
        }
        Ok(())
    }
}
fn validate_limits(limits: &Limits) -> Result<(), AgentError> {
    if limits.timeout.is_zero()
        || limits.timeout > Duration::from_secs(86400)
        || limits.max_steps == 0
        || limits.max_text_bytes == 0
    {
        return Err(AgentError::Configuration("invalid mailbox turn budget"));
    }
    Ok(())
}
impl<C: MailboxClock> Mailbox<'_, C> {
    pub fn process_one(
        &self,
        worker: &str,
        parent: &TurnContext,
        limits: Limits,
        host: &mut impl MailboxExecutor,
    ) -> Result<Option<DeliveryResult>, AgentError> {
        validate_limits(&limits)?;
        let context = parent.child(limits.timeout)?;
        let lease_for = context
            .remaining()?
            .saturating_add(Duration::from_secs(1))
            .min(Duration::from_secs(86400));
        let Some(lease) = self.claim(worker, lease_for)? else {
            return Ok(None);
        };
        self.process_claimed(&lease, &context, limits, host)
            .map(Some)
    }
    /// Consume only this still-current lease; setup/queue time stays on the parent clock.
    pub fn process_claimed(
        &self,
        lease: &Lease,
        parent: &TurnContext,
        limits: Limits,
        host: &mut impl MailboxExecutor,
    ) -> Result<DeliveryResult, AgentError> {
        validate_limits(&limits)?;
        let context = parent.child(limits.timeout)?;
        let run = self.registry.start_task_checked(
            &self.owner,
            &self.pal_id,
            &lease.task_id,
            |record| self.validate_lease(record, lease, true).map(|_| ()),
        )?;
        let result = host.execute(
            run,
            &context,
            limits,
            &mut LeaseObserver {
                mailbox: self,
                lease,
                host: &mut namzu_core::NoopObserver,
            },
        );
        let delivery = self.ack(lease)?;
        Ok(DeliveryResult {
            message_id: lease.message_id.clone(),
            task_id: lease.task_id.clone(),
            delivery,
            result,
        })
    }
    /// Explicitly start a bounded automatic pump: stops at idle, count, error or cancellation.
    /// Does not spawn a detached daemon or retry failed model/tool work.
    pub fn drain(
        &self,
        worker: &str,
        parent: &TurnContext,
        budget: BatchBudget,
        host: &mut impl MailboxExecutor,
    ) -> Result<BatchResult, AgentError> {
        budget.validate()?;
        let context = parent.child(budget.total.timeout)?;
        let mut batch = BatchResult {
            deliveries: vec![],
            stopped: None,
        };
        for _ in 0..budget.max_messages {
            match self.process_one(worker, &context, budget.per_message.clone(), host) {
                Ok(Some(delivery)) => batch.deliveries.push(delivery),
                Ok(None) => break,
                Err(error) => {
                    batch.stopped = Some(error);
                    break;
                }
            }
        }
        Ok(batch)
    }
    fn fence(&self, lease: &Lease) -> Result<(), AgentError> {
        let record = self.registry.load(&self.owner, &self.pal_id)?;
        self.validate_lease(&record, lease, true)?;
        if find_task(&record, &lease.task_id)?.status != TaskStatus::Running {
            return Err(AgentError::State("delivery no longer owns a running task"));
        }
        Ok(())
    }
}
struct LeaseObserver<'a, 'r, C, O> {
    mailbox: &'a Mailbox<'r, C>,
    lease: &'a Lease,
    host: &'a mut O,
}
impl<C: MailboxClock, O: Observer> Observer for LeaseObserver<'_, '_, C, O> {
    fn supports_approval(&self) -> bool {
        self.host.supports_approval()
    }
    fn approve(&mut self, call: &ToolCall, context: &TurnContext) -> Result<bool, AgentError> {
        self.mailbox.fence(self.lease)?;
        let approved = self.host.approve(call, context)?;
        self.mailbox.fence(self.lease)?;
        Ok(approved)
    }
    fn event(&mut self, event: Event) -> Result<(), AgentError> {
        let boundary = matches!(
            event,
            Event::ProviderStarted { .. } | Event::ToolStarted { .. }
        );
        self.host.event(event)?;
        if boundary {
            self.mailbox.fence(self.lease)?;
        }
        Ok(())
    }
    fn checkpoint(
        &mut self,
        messages: &[Message],
        stats: &Stats,
        pending: Option<&ToolCall>,
    ) -> Result<(), AgentError> {
        self.host.checkpoint(messages, stats, pending)?;
        self.mailbox.fence(self.lease)
    }
}
