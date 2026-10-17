//! Provider-independent SDK. No network, shell, credentials or implicit tools.
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
}

/// Shared with provider/tool implementations. Blocking operations must enforce
/// `remaining()` themselves. Cancellation is checked at every effect boundary.
#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

pub struct TurnContext {
    cancellation: Cancellation,
    deadline: Instant,
}
impl TurnContext {
    pub fn remaining(&self) -> Result<Duration, AgentError> {
        if self.cancellation.is_cancelled() {
            return Err(AgentError::Cancelled);
        }
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or(AgentError::Timeout)
    }
}

pub trait Provider {
    fn complete(
        &mut self,
        messages: &[Message],
        tools: &[ToolDefinition],
        context: &TurnContext,
    ) -> Result<Completion, AgentError>;
}

pub trait Tools {
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
    if limits.timeout.is_zero() || limits.max_steps == 0 || limits.max_text_bytes == 0 {
        return Err(AgentError::Configuration(
            "positive timeout, steps and text budget required",
        ));
    }
    let deadline = Instant::now()
        .checked_add(limits.timeout)
        .ok_or(AgentError::Configuration("timeout overflow"))?;
    let context = TurnContext {
        deadline,
        cancellation,
    };
    let mut remaining_bytes = limits.max_text_bytes;
    let mut account = |bytes: usize| -> Result<(), AgentError> {
        remaining_bytes = remaining_bytes
            .checked_sub(bytes)
            .ok_or(AgentError::Protocol("conversation exceeds byte budget"))?;
        Ok(())
    };
    account(prompt.len())?;
    let mut messages = vec![Message::User {
        content: prompt.into(),
    }];
    let mut ids = HashSet::new();
    let mut stats = Stats {
        provider_calls: 0,
        tool_calls: 0,
    };
    for _ in 0..limits.max_steps {
        context.remaining()?;
        let definitions = tools.definitions();
        let names: HashSet<_> = definitions.iter().map(|d| d.name.as_str()).collect();
        if names.len() != definitions.len() {
            return Err(AgentError::Configuration("duplicate tool definitions"));
        }
        let response = provider.complete(&messages, &definitions, &context)?;
        stats.provider_calls += 1;
        context.remaining()?;
        account(response.text.len())?;
        if response.tool_calls.is_empty() {
            messages.push(Message::Assistant {
                content: response.text.clone(),
                tool_calls: vec![],
            });
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
        messages.push(Message::Assistant {
            content: response.text,
            tool_calls: response.tool_calls.clone(),
        });
        for call in response.tool_calls {
            context.remaining()?;
            let result = if names.contains(call.name.as_str()) {
                tools.call(&call, &context)?
            } else {
                ToolResult::error("tool is unavailable")
            };
            stats.tool_calls += 1;
            context.remaining()?;
            account(result.content.len())?;
            messages.push(Message::Tool {
                tool_call_id: call.id,
                content: result.content,
                is_error: result.is_error,
            });
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
    fn expired_context_returns_timeout_without_racing_a_timer() {
        let context = TurnContext {
            cancellation: Cancellation::default(),
            deadline: Instant::now().checked_sub(Duration::from_secs(1)).unwrap(),
        };
        assert_eq!(context.remaining(), Err(AgentError::Timeout));
    }
}
