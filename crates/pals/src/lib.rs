//! Explicit child-agent SDK. No implicit provider, credentials, shell or global state.
mod registry;
mod team;
use namzu_core::{
    AgentError, Limits, Message, Observer, Provider, ToolCall, ToolDefinition, ToolResult, Tools,
    Turn, TurnContext, run_history_in_context,
};
pub use registry::{
    BatchBudget, BatchResult, DeliveryResult, DeliveryState, Envelope, HostRuntime, Lease,
    MailMessage, MailSender, Mailbox, MailboxClock, MailboxHost, MailboxRuntime, PalCreate,
    PalDefinition, PalRun, PalStatus, PalUpdate, Registry, SendReceipt, SystemClock, TaskRecord,
    TaskStatus,
};
use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
pub use team::{Job, Team, TeamControl};

static SCOPES: AtomicU64 = AtomicU64::new(1);
const READ_ONLY: &[&str] = &["read_file", "list_files", "search_files", "diff_file"];

pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
}

/// Explicit allocation for delegation, supplied from the host's remaining budget.
/// Reservations are atomic and never refunded, even on failure/drop: retries cannot
/// manufacture additional budget. This is NOT the parent's original turn budget.
pub struct Delegation {
    pub(crate) id: u64,
    owner: String,
    context: TurnContext,
    ceiling: Limits,
    admitted: BTreeSet<String>,
    reserved: Mutex<Reserved>,
}
#[derive(Default)]
struct Reserved {
    steps: usize,
    tools: usize,
    bytes: usize,
    ids: BTreeSet<String>,
}

#[derive(Clone)]
pub struct TaskSpec {
    pub id: String,
    pub prompt: String,
    /// None selects only known read-only tools, intersected with parent admission.
    pub tools: Option<BTreeSet<String>>,
    pub limits: Limits,
}
impl TaskSpec {
    pub fn new(id: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            prompt: prompt.into(),
            tools: None,
            limits: Limits {
                timeout: Duration::from_secs(300),
                max_steps: 16,
                max_tool_calls: 16,
                max_text_bytes: 256 * 1024,
            },
        }
    }
}

impl Delegation {
    pub fn new(
        owner: &str,
        parent: &TurnContext,
        remaining: Limits,
        tools: &impl Tools,
    ) -> Result<Self, AgentError> {
        if !valid_id(owner) || remaining.max_steps == 0 || remaining.max_text_bytes == 0 {
            return Err(AgentError::Configuration(
                "invalid owner or delegation budget",
            ));
        }
        let context = parent.child(remaining.timeout)?;
        let admitted = tools.definitions().into_iter().map(|d| d.name).collect();
        Ok(Self {
            id: SCOPES.fetch_add(1, Ordering::Relaxed),
            owner: owner.into(),
            context,
            ceiling: remaining,
            admitted,
            reserved: Mutex::new(Reserved::default()),
        })
    }
    pub fn cancellation(&self) -> namzu_core::Cancellation {
        self.context.cancellation()
    }
    pub fn prepare(&self, spec: TaskSpec) -> Result<ChildTask, AgentError> {
        self.context.remaining()?;
        if !valid_id(&spec.id)
            || spec.prompt.is_empty()
            || spec.prompt.len() > 16 * 1024
            || spec.limits.max_steps == 0
            || spec.limits.max_text_bytes == 0
        {
            return Err(AgentError::Configuration(
                "invalid task identity, prompt or budget",
            ));
        }
        let names = spec
            .tools
            .unwrap_or_else(|| READ_ONLY.iter().map(|s| (*s).into()).collect());
        // Intersection is intentional: a requested capability never expands admission.
        let admitted = names.intersection(&self.admitted).cloned().collect();
        let mut limits = spec.limits;
        limits.timeout = limits.timeout.min(self.ceiling.timeout);
        let context = self.context.child(limits.timeout)?;
        let mut reserved = self
            .reserved
            .lock()
            .map_err(|_| AgentError::State("delegation lock poisoned"))?;
        if reserved.ids.contains(&spec.id) || reserved.ids.len() >= 64 {
            return Err(AgentError::Configuration(
                "duplicate task ID or task count limit",
            ));
        }
        if limits.max_steps > self.ceiling.max_steps.saturating_sub(reserved.steps) {
            return Err(AgentError::StepLimit);
        }
        if limits.max_tool_calls > self.ceiling.max_tool_calls.saturating_sub(reserved.tools) {
            return Err(AgentError::ToolLimit);
        }
        if limits.max_text_bytes > self.ceiling.max_text_bytes.saturating_sub(reserved.bytes) {
            return Err(AgentError::Configuration(
                "delegation byte budget exhausted",
            ));
        }
        reserved.steps += limits.max_steps;
        reserved.tools += limits.max_tool_calls;
        reserved.bytes += limits.max_text_bytes;
        reserved.ids.insert(spec.id.clone());
        Ok(ChildTask {
            scope: self.id,
            owner: self.owner.clone(),
            id: spec.id,
            prompt: spec.prompt,
            context,
            limits,
            admitted,
        })
    }
}

/// Single-use child: fresh conversation, private cancellation and fixed admission.
pub struct ChildTask {
    pub(crate) scope: u64,
    owner: String,
    id: String,
    prompt: String,
    context: TurnContext,
    limits: Limits,
    admitted: BTreeSet<String>,
}
pub struct TaskResult {
    pub id: String,
    pub owner: String,
    pub result: Result<Turn, AgentError>,
}
impl ChildTask {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn owner(&self) -> &str {
        &self.owner
    }
    pub fn scope_id(&self) -> u64 {
        self.scope
    }
    pub fn cancellation(&self) -> namzu_core::Cancellation {
        self.context.cancellation()
    }
    pub fn run(
        self,
        provider: &mut impl Provider,
        tools: &mut impl Tools,
        observer: &mut impl Observer,
    ) -> TaskResult {
        let mut tools = RestrictedTools {
            inner: tools,
            admitted: self.admitted,
        };
        let result = run_history_in_context(
            provider,
            &mut tools,
            vec![Message::User {
                content: self.prompt,
            }],
            self.limits,
            &self.context,
            observer,
        );
        TaskResult {
            id: self.id,
            owner: self.owner,
            result,
        }
    }
}
struct RestrictedTools<'a, T> {
    inner: &'a mut T,
    admitted: BTreeSet<String>,
}
impl<T: Tools> Tools for RestrictedTools<'_, T> {
    fn definitions(&self) -> Vec<ToolDefinition> {
        self.inner
            .definitions()
            .into_iter()
            .filter(|d| self.admitted.contains(&d.name))
            .collect()
    }
    fn call(&mut self, call: &ToolCall, context: &TurnContext) -> Result<ToolResult, AgentError> {
        context.remaining()?;
        if !self.admitted.contains(&call.name) {
            return Ok(ToolResult::error("child tool denied"));
        }
        self.inner.call(call, context)
    }
}

// Keep shared ownership explicit for embedding runtimes.
pub type SharedDelegation = Arc<Delegation>;
