use crate::{AgentError, Stats, ToolCall, ToolResult, TurnContext};
use serde::{Deserialize, Serialize};

/// Presentation events contain task content, never opaque provider replay state.
/// Deltas are provisional: only AssistantMessage accepts a completed response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    TurnStarted,
    ProviderStarted {
        step: usize,
    },
    ProviderFinished {
        metadata: crate::ResponseMetadata,
    },
    AssistantDelta {
        text: String,
    },
    AssistantMessage {
        text: String,
        has_tool_calls: bool,
    },
    ToolStarted {
        id: String,
        name: String,
    },
    ToolFinished {
        id: String,
        name: String,
        result: ToolResult,
    },
    TurnCompleted {
        stats: Stats,
    },
    TurnFailed {
        error: String,
    },
}

/// A disconnected consumer may stop the turn. Hosts must keep callbacks bounded;
/// they run synchronously and do not replace durable pre-effect checkpoints.
pub trait EventSink {
    /// True only for hosts that actually present permission prompts.
    fn supports_approval(&self) -> bool {
        false
    }
    /// Optional validated call presentation hook. Never authorizes an effect.
    fn tool_proposed(&mut self, _: &ToolCall) -> Result<(), AgentError> {
        Ok(())
    }
    /// Host permission decision; must stay inside existing tool-policy ceilings.
    fn approve(&mut self, _: &ToolCall, _: &TurnContext) -> Result<bool, AgentError> {
        Ok(true)
    }
    fn emit(&mut self, event: Event) -> Result<(), AgentError>;
}

pub struct NoopEvents;
impl EventSink for NoopEvents {
    fn emit(&mut self, _: Event) -> Result<(), AgentError> {
        Ok(())
    }
}
