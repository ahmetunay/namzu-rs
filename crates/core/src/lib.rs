//! Provider-independent SDK. No network, shell, credentials or implicit tools.
mod events;
pub use events::{Event, EventSink, NoopEvents};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    /// Opaque provider replay data, never promoted to system/user instructions.
    ProviderState {
        data: Value,
    },
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        content: String,
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        tool_call_id: String,
        content: String,
        is_error: bool,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Clone, Debug, Default)]
pub struct Completion {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
}

impl ToolResult {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
        }
    }
    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum AgentError {
    #[error("operation cancelled")]
    Cancelled,
    #[error("turn deadline exceeded")]
    Timeout,
    #[error("step budget exhausted")]
    StepLimit,
    #[error("tool call budget exhausted")]
    ToolLimit,
    #[error("invalid configuration: {0}")]
    Configuration(&'static str),
    #[error("invalid provider response: {0}")]
    Protocol(&'static str),
    #[error("provider failure: {0}")]
    Provider(String),
    #[error("state failure: {0}")]
    State(&'static str),
}

/// Shared with provider/tool implementations. Blocking operations must enforce
/// `remaining()` themselves. Cancellation is checked at every effect boundary.
#[derive(Default)]
struct CancellationNode {
    cancelled: AtomicBool,
    parent: Option<Arc<CancellationNode>>,
}
#[derive(Clone, Default)]
pub struct Cancellation(Arc<CancellationNode>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        let mut node = Some(self.0.as_ref());
        while let Some(current) = node {
            if current.cancelled.load(Ordering::Acquire) {
                return true;
            }
            node = current.parent.as_deref();
        }
        false
    }
    /// Downward propagation only; cancelling a child leaves its parent/siblings live.
    pub fn child(&self) -> Self {
        Self(Arc::new(CancellationNode {
            cancelled: AtomicBool::new(false),
            parent: Some(self.0.clone()),
        }))
    }
}

#[derive(Clone)]
pub struct TurnContext {
    cancellation: Cancellation,
    deadline: Instant,
}
impl TurnContext {
    pub fn new(timeout: Duration, cancellation: Cancellation) -> Result<Self, AgentError> {
        if timeout.is_zero() {
            return Err(AgentError::Configuration("positive timeout required"));
        }
        Ok(Self {
            cancellation,
            deadline: Instant::now()
                .checked_add(timeout)
                .ok_or(AgentError::Configuration("timeout overflow"))?,
        })
    }
    /// Prove a caller retained this context's exact cancellation ancestry and
    /// did not extend its deadline. Clones and children satisfy this relation.
    pub fn is_bounded_by(&self, parent: &Self) -> bool {
        if self.deadline > parent.deadline {
            return false;
        }
        let mut node = Some(&self.cancellation.0);
        while let Some(current) = node {
            if Arc::ptr_eq(current, &parent.cancellation.0) {
                return true;
            }
            node = current.parent.as_ref();
        }
        false
    }
    pub fn remaining(&self) -> Result<Duration, AgentError> {
        if self.cancellation.is_cancelled() {
            return Err(AgentError::Cancelled);
        }
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or(AgentError::Timeout)
    }
    pub fn cancellation(&self) -> Cancellation {
        self.cancellation.clone()
    }
    /// Preserve the exact parent deadline, including time spent queued/setting up.
    pub fn child(&self, timeout: Duration) -> Result<Self, AgentError> {
        self.remaining()?;
        let mut child = Self::new(timeout, self.cancellation.child())?;
        child.deadline = child.deadline.min(self.deadline);
        Ok(child)
    }
}

pub trait Provider {
    fn take_state(&mut self) -> Option<Value> {
        None
    }
    fn complete(
        &mut self,
        messages: &[Message],
        tools: &[ToolDefinition],
        context: &TurnContext,
    ) -> Result<Completion, AgentError>;

    /// Streaming is optional. The fallback emits the validated buffered text.
    fn complete_stream(
        &mut self,
        messages: &[Message],
        tools: &[ToolDefinition],
        context: &TurnContext,
        events: &mut dyn EventSink,
    ) -> Result<Completion, AgentError> {
        let response = self.complete(messages, tools, context)?;
        context.remaining()?;
        if !response.text.is_empty() {
            events.emit(Event::AssistantDelta {
                text: response.text.clone(),
            })?;
        }
        Ok(response)
    }
}

pub trait Tools {
    /// Validate a complete response batch before the first tool effect.
    fn validate(&mut self, _: &ToolCall, context: &TurnContext) -> Result<(), AgentError> {
        context.remaining().map(|_| ())
    }
    fn definitions(&self) -> Vec<ToolDefinition>;
    fn call(&mut self, call: &ToolCall, context: &TurnContext) -> Result<ToolResult, AgentError>;
}

#[derive(Clone, Debug)]
pub struct Limits {
    pub timeout: Duration,
    pub max_steps: usize,
    pub max_tool_calls: usize,
    pub max_text_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(300),
            max_steps: 64,
            max_tool_calls: 128,
            max_text_bytes: 4 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Stats {
    pub provider_calls: usize,
    pub tool_calls: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Turn {
    pub output: String,
    pub stats: Stats,
    pub messages: Vec<Message>,
}

/// Durable host checkpoint. `pending_call` is saved BEFORE a tool is invoked.
/// A host must refuse automatic replay after an interrupted/uncertain effect.
pub trait Observer {
    fn supports_approval(&self) -> bool {
        false
    }
    fn approve(&mut self, _: &ToolCall, _: &TurnContext) -> Result<bool, AgentError> {
        Ok(true)
    }
    fn event(&mut self, _: Event) -> Result<(), AgentError> {
        Ok(())
    }
    fn checkpoint(
        &mut self,
        messages: &[Message],
        stats: &Stats,
        pending_call: Option<&ToolCall>,
    ) -> Result<(), AgentError>;
}
pub struct NoopObserver;
impl Observer for NoopObserver {
    fn checkpoint(
        &mut self,
        _: &[Message],
        _: &Stats,
        _: Option<&ToolCall>,
    ) -> Result<(), AgentError> {
        Ok(())
    }
}

/// Run a single turn. The complete tool-call batch is validated and budgeted
/// before its first effect. Tool failures are conversation data; cancellation,
/// malformed calls and exhausted budgets are turn failures.
pub fn run_turn(
    provider: &mut impl Provider,
    tools: &mut impl Tools,
    prompt: &str,
    limits: Limits,
    cancellation: Cancellation,
) -> Result<Turn, AgentError> {
    run_history(
        provider,
        tools,
        vec![Message::User {
            content: prompt.into(),
        }],
        limits,
        cancellation,
        &mut NoopObserver,
    )
}

pub fn run_history(
    provider: &mut impl Provider,
    tools: &mut impl Tools,
    messages: Vec<Message>,
    limits: Limits,
    cancellation: Cancellation,
    observer: &mut impl Observer,
) -> Result<Turn, AgentError> {
    let context = TurnContext::new(limits.timeout, cancellation)?;
    run_history_in_context(provider, tools, messages, limits, &context, observer)
}

/// Run under an existing deadline/cancellation ceiling, without resetting its clock.
pub fn run_history_in_context(
    provider: &mut impl Provider,
    tools: &mut impl Tools,
    messages: Vec<Message>,
    limits: Limits,
    parent: &TurnContext,
    observer: &mut impl Observer,
) -> Result<Turn, AgentError> {
    let context = parent.child(limits.timeout)?;
    observer.event(Event::TurnStarted)?;
    let result = run_history_inner(provider, tools, messages, limits, context, observer);
    match &result {
        Ok(turn) => observer.event(Event::TurnCompleted {
            stats: turn.stats.clone(),
        })?,
        Err(error) => {
            let _ = observer.event(Event::TurnFailed {
                error: error.to_string(),
            });
        }
    }
    result
}

struct ObserverEvents<'a, T>(&'a mut T);
impl<T: Observer> EventSink for ObserverEvents<'_, T> {
    fn emit(&mut self, event: Event) -> Result<(), AgentError> {
        self.0.event(event)
    }
}

fn run_history_inner(
    provider: &mut impl Provider,
    tools: &mut impl Tools,
    mut messages: Vec<Message>,
    limits: Limits,
    context: TurnContext,
    observer: &mut impl Observer,
) -> Result<Turn, AgentError> {
    if limits.timeout.is_zero() || limits.max_steps == 0 || limits.max_text_bytes == 0 {
        return Err(AgentError::Configuration(
            "positive timeout, steps and text budget required",
        ));
    }
    let mut remaining_bytes = limits.max_text_bytes;
    let mut account = |bytes: usize| -> Result<(), AgentError> {
        remaining_bytes = remaining_bytes
            .checked_sub(bytes)
            .ok_or(AgentError::Protocol("conversation exceeds byte budget"))?;
        Ok(())
    };
    account(
        serde_json::to_vec(&messages)
            .map_err(|_| AgentError::Protocol("cannot encode history"))?
            .len(),
    )?;
    let mut ids = HashSet::new();
    let mut pending = HashSet::new();
    for message in &messages {
        match message {
            Message::Assistant { tool_calls, .. } => {
                if !pending.is_empty() {
                    return Err(AgentError::Protocol("unfinished tool history"));
                }
                for call in tool_calls {
                    if call.id.is_empty() || !ids.insert(call.id.clone()) {
                        return Err(AgentError::Protocol("invalid historical tool call ID"));
                    }
                    pending.insert(call.id.clone());
                }
            }
            Message::Tool { tool_call_id, .. } => {
                if !pending.remove(tool_call_id) {
                    return Err(AgentError::Protocol("orphan historical tool result"));
                }
            }
            _ => {
                if !pending.is_empty() {
                    return Err(AgentError::Protocol("unfinished tool history"));
                }
            }
        }
    }
    if !pending.is_empty() {
        return Err(AgentError::Protocol(
            "unresolved historical tool calls; explicit recovery required",
        ));
    }
    let mut stats = Stats {
        provider_calls: 0,
        tool_calls: 0,
    };
    observer.checkpoint(&messages, &stats, None)?;
    for _ in 0..limits.max_steps {
        context.remaining()?;
        let definitions = tools.definitions();
        let names: HashSet<_> = definitions.iter().map(|d| d.name.as_str()).collect();
        if names.len() != definitions.len() {
            return Err(AgentError::Configuration("duplicate tool definitions"));
        }
        observer.event(Event::ProviderStarted {
            step: stats.provider_calls + 1,
        })?;
        context.remaining()?;
        let response = provider.complete_stream(
            &messages,
            &definitions,
            &context,
            &mut ObserverEvents(observer),
        )?;
        if let Some(data) = provider.take_state() {
            account(data.to_string().len())?;
            messages.push(Message::ProviderState { data });
        }
        stats.provider_calls += 1;
        context.remaining()?;
        account(response.text.len())?;
        if response.tool_calls.is_empty() {
            messages.push(Message::Assistant {
                content: response.text.clone(),
                tool_calls: vec![],
            });
            observer.checkpoint(&messages, &stats, None)?;
            observer.event(Event::AssistantMessage {
                text: response.text.clone(),
                has_tool_calls: false,
            })?;
            context.remaining()?;
            return Ok(Turn {
                output: response.text,
                stats,
                messages,
            });
        }
        if response.tool_calls.len() > limits.max_tool_calls.saturating_sub(stats.tool_calls) {
            return Err(AgentError::ToolLimit);
        }
        for call in &response.tool_calls {
            if call.id.is_empty() || call.id.len() > 256 || !ids.insert(call.id.clone()) {
                return Err(AgentError::Protocol(
                    "empty, excessive or reused tool call ID",
                ));
            }
            if call.name.len() > 128 || !call.arguments.is_object() {
                return Err(AgentError::Protocol(
                    "invalid tool name or non-object arguments",
                ));
            }
            account(call.id.len() + call.name.len() + call.arguments.to_string().len())?;
        }
        for call in &response.tool_calls {
            if names.contains(call.name.as_str()) {
                tools.validate(call, &context)?;
            }
        }
        messages.push(Message::Assistant {
            content: response.text.clone(),
            tool_calls: response.tool_calls.clone(),
        });
        observer.event(Event::AssistantMessage {
            text: response.text,
            has_tool_calls: true,
        })?;
        for call in response.tool_calls {
            context.remaining()?;
            let approved =
                !names.contains(call.name.as_str()) || observer.approve(&call, &context)?;
            context.remaining()?;
            observer.checkpoint(&messages, &stats, Some(&call))?;
            observer.event(Event::ToolStarted {
                id: call.id.clone(),
                name: call.name.clone(),
            })?;
            context.remaining()?;
            let result = if !approved {
                ToolResult::error("tool call denied by user")
            } else if names.contains(call.name.as_str()) {
                tools.call(&call, &context)?
            } else {
                ToolResult::error("tool is unavailable")
            };
            stats.tool_calls += 1;
            context.remaining()?;
            account(result.content.len())?;
            let event = Event::ToolFinished {
                id: call.id.clone(),
                name: call.name,
                result: result.clone(),
            };
            messages.push(Message::Tool {
                tool_call_id: call.id,
                content: result.content,
                is_error: result.is_error,
            });
            observer.checkpoint(&messages, &stats, None)?;
            observer.event(event)?;
        }
    }
    Err(AgentError::StepLimit)
}

/// Offline fixture provider: never accesses a model or account.
pub struct ScriptedProvider(pub std::collections::VecDeque<Completion>);
impl Provider for ScriptedProvider {
    fn complete(
        &mut self,
        _: &[Message],
        _: &[ToolDefinition],
        context: &TurnContext,
    ) -> Result<Completion, AgentError> {
        context.remaining()?;
        self.0
            .pop_front()
            .ok_or_else(|| AgentError::Provider("script exhausted".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inherited_deadline_is_exact_and_cancellation_is_downward() {
        let parent = TurnContext::new(Duration::from_secs(30), Cancellation::default()).unwrap();
        let child = parent.child(Duration::from_secs(300)).unwrap();
        assert_eq!(parent.deadline, child.deadline);
        let grandchild = child.child(Duration::from_secs(300)).unwrap();
        assert_eq!(parent.deadline, grandchild.deadline);
        child.cancellation().cancel();
        assert_eq!(grandchild.remaining(), Err(AgentError::Cancelled));
        assert!(parent.remaining().is_ok());
    }
    #[test]
    fn context_bounds_require_both_deadline_and_cancellation_ancestry() {
        let root = TurnContext::new(Duration::from_secs(30), Cancellation::default()).unwrap();
        assert!(root.clone().is_bounded_by(&root));
        let child = root.child(Duration::from_secs(1)).unwrap();
        assert!(child.is_bounded_by(&root));
        assert!(!root.is_bounded_by(&child));
        let independent =
            TurnContext::new(Duration::from_millis(1), Cancellation::default()).unwrap();
        assert!(!independent.is_bounded_by(&root));
        let extended = TurnContext::new(Duration::from_secs(60), root.cancellation()).unwrap();
        assert!(!extended.is_bounded_by(&root));
        child.cancellation().cancel();
        assert!(root.remaining().is_ok());
    }
    #[test]
    fn expired_context_returns_timeout_without_racing_a_timer() {
        let context = TurnContext {
            cancellation: Cancellation::default(),
            deadline: Instant::now().checked_sub(Duration::from_secs(1)).unwrap(),
        };
        assert_eq!(context.remaining(), Err(AgentError::Timeout));
    }

    #[test]
    fn time_spent_delivering_turn_started_is_charged_before_provider_work() {
        struct NoTools;
        impl Tools for NoTools {
            fn definitions(&self) -> Vec<ToolDefinition> {
                vec![]
            }
            fn call(&mut self, _: &ToolCall, _: &TurnContext) -> Result<ToolResult, AgentError> {
                panic!("no effects after expired event delivery")
            }
        }
        let mut provider = ScriptedProvider(
            vec![Completion {
                text: "unused".into(),
                tool_calls: vec![],
            }]
            .into(),
        );
        let result = run_history_inner(
            &mut provider,
            &mut NoTools,
            vec![Message::User {
                content: "test".into(),
            }],
            Limits {
                timeout: Duration::from_secs(1),
                ..Limits::default()
            },
            TurnContext {
                cancellation: Cancellation::default(),
                deadline: Instant::now().checked_sub(Duration::from_secs(2)).unwrap(),
            },
            &mut NoopObserver,
        );
        assert_eq!(result.unwrap_err(), AgentError::Timeout);
        assert_eq!(provider.0.len(), 1);
    }
}
