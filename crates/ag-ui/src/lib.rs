//! Bounded AG-UI 0.0.59-compatible event subset. Task content, never telemetry.
//! Native terminal events are provisional: the host settles persistence before finish.
use namzu_core::{AgentError, Event, ToolCall, ToolResult, Turn};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
pub mod client;
pub const MAX_INPUT: usize = 256 * 1024;
pub const MAX_FRAME: usize = 1024 * 1024;
pub const MAX_STREAM: usize = 16 * 1024 * 1024;
fn bad() -> AgentError {
    AgentError::Protocol("invalid or excessive AG-UI lifecycle")
}
pub fn valid_id(id: &str) -> bool {
    !id.trim().is_empty() && id.len() <= 256 && !id.chars().any(char::is_control)
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunInput {
    pub thread_id: String,
    pub run_id: String,
    pub messages: Vec<Value>,
    #[serde(default)]
    pub tools: Vec<Value>,
    #[serde(default)]
    pub context: Vec<Value>,
    #[serde(default)]
    pub state: Value,
    #[serde(default)]
    pub forwarded_props: Value,
}
impl RunInput {
    /// Admit only the last user text. Prior display history is never promoted to native
    /// history; the host owns that history. Client tools/state/context cannot grant scope.
    pub fn admit(&self) -> Result<(&str, &str), AgentError> {
        let empty = |v: &Value| v.is_null() || v.as_object().is_some_and(|m| m.is_empty());
        if !valid_id(&self.thread_id)
            || !valid_id(&self.run_id)
            || self.messages.is_empty()
            || self.messages.len() > 1024
            || !self.tools.is_empty()
            || !self.context.is_empty()
            || !empty(&self.state)
            || !empty(&self.forwarded_props)
            || self
                .messages
                .iter()
                .any(|m| !matches!(m["role"].as_str(), Some("user" | "assistant" | "tool")))
        {
            return Err(bad());
        }
        let m = self.messages.last().unwrap();
        let (Some(id), Some(content)) = (m["id"].as_str(), m["content"].as_str()) else {
            return Err(bad());
        };
        if m["role"] != "user"
            || !valid_id(id)
            || content.trim().is_empty()
            || content.len() > 64 * 1024
        {
            return Err(bad());
        }
        Ok((id, content))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum WireEvent {
    #[serde(rename = "RUN_STARTED")]
    RunStarted {
        #[serde(rename = "threadId")]
        thread_id: String,
        #[serde(rename = "runId")]
        run_id: String,
    },
    #[serde(rename = "RUN_FINISHED")]
    RunFinished {
        #[serde(rename = "threadId")]
        thread_id: String,
        #[serde(rename = "runId")]
        run_id: String,
        result: Value,
    },
    #[serde(rename = "RUN_ERROR")]
    RunError { message: String, code: String },
    #[serde(rename = "STEP_STARTED")]
    StepStarted {
        #[serde(rename = "stepName")]
        step_name: String,
    },
    #[serde(rename = "STEP_FINISHED")]
    StepFinished {
        #[serde(rename = "stepName")]
        step_name: String,
    },
    #[serde(rename = "TEXT_MESSAGE_START")]
    TextStart {
        #[serde(rename = "messageId")]
        message_id: String,
        role: String,
    },
    #[serde(rename = "TEXT_MESSAGE_CONTENT")]
    TextContent {
        #[serde(rename = "messageId")]
        message_id: String,
        delta: String,
    },
    #[serde(rename = "TEXT_MESSAGE_END")]
    TextEnd {
        #[serde(rename = "messageId")]
        message_id: String,
    },
    #[serde(rename = "TOOL_CALL_START")]
    ToolStart {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolCallName")]
        tool_call_name: String,
    },
    #[serde(rename = "TOOL_CALL_ARGS")]
    ToolArgs {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        delta: String,
    },
    #[serde(rename = "TOOL_CALL_END")]
    ToolEnd {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
    },
    #[serde(rename = "TOOL_CALL_RESULT")]
    ToolResult {
        #[serde(rename = "messageId")]
        message_id: String,
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        role: String,
        content: String,
        metadata: Value,
    },
    #[serde(rename = "CUSTOM")]
    Custom { name: String, value: Value },
}
pub fn sse(event: &WireEvent) -> Result<Vec<u8>, AgentError> {
    let bytes = serde_json::to_vec(event).map_err(|_| bad())?;
    if bytes.len() > MAX_FRAME {
        return Err(bad());
    }
    let mut result = b"data: ".to_vec();
    result.extend(bytes);
    result.extend(b"\n\n");
    Ok(result)
}
struct Tool {
    fingerprint: [u8; 32],
    result: bool,
}
pub struct Mapper {
    thread: String,
    run: String,
    started: bool,
    terminal: bool,
    message: Option<(String, String)>,
    message_count: u64,
    step: Option<String>,
    tools: HashMap<String, Tool>,
    bytes: usize,
}
impl Mapper {
    pub fn new(thread: &str, run: &str) -> Result<Self, AgentError> {
        if !valid_id(thread) || !valid_id(run) {
            return Err(bad());
        }
        Ok(Self {
            thread: thread.into(),
            run: run.into(),
            started: false,
            terminal: false,
            message: None,
            message_count: 0,
            step: None,
            tools: HashMap::new(),
            bytes: 0,
        })
    }
    fn account(&mut self, events: Vec<WireEvent>) -> Result<Vec<WireEvent>, AgentError> {
        let n = events
            .iter()
            .try_fold(0usize, |a, e| a.checked_add(sse(e)?.len()).ok_or_else(bad))?;
        if n > MAX_STREAM.saturating_sub(8192).saturating_sub(self.bytes) {
            return Err(bad());
        }
        self.bytes += n;
        Ok(events)
    }
    pub fn start(&mut self) -> Result<Vec<WireEvent>, AgentError> {
        if self.terminal || self.started {
            return Ok(vec![]);
        }
        self.started = true;
        self.account(vec![WireEvent::RunStarted {
            thread_id: self.thread.clone(),
            run_id: self.run.clone(),
        }])
    }
    fn text_start(&mut self, out: &mut Vec<WireEvent>) {
        if self.message.is_none() {
            self.message_count += 1;
            let id = format!("m-{}-{}", self.run, self.message_count);
            out.push(WireEvent::TextStart {
                message_id: id.clone(),
                role: "assistant".into(),
            });
            self.message = Some((id, String::new()));
        }
    }
    fn text_end(&mut self, out: &mut Vec<WireEvent>) {
        if let Some((id, _)) = self.message.take() {
            out.push(WireEvent::TextEnd { message_id: id });
        }
    }
    fn step_end(&mut self, out: &mut Vec<WireEvent>) {
        if let Some(name) = self.step.take() {
            out.push(WireEvent::StepFinished { step_name: name });
        }
    }
    pub fn map(&mut self, event: &Event) -> Result<Vec<WireEvent>, AgentError> {
        if self.terminal {
            return Ok(vec![]);
        }
        let mut out = self.start()?;
        match event {
            Event::ProviderStarted { step } => {
                self.text_end(&mut out);
                self.step_end(&mut out);
                let name = format!("provider-{step}");
                self.step = Some(name.clone());
                out.push(WireEvent::StepStarted { step_name: name });
            }
            Event::AssistantDelta { text } => {
                if !text.is_empty() {
                    self.text_start(&mut out);
                    let (id, retained) = self.message.as_mut().unwrap();
                    if text.len() > 4 * 1024 * 1024 - retained.len() {
                        return Err(bad());
                    }
                    retained.push_str(text);
                    out.push(WireEvent::TextContent {
                        message_id: id.clone(),
                        delta: text.clone(),
                    });
                }
            }
            Event::AssistantMessage { text, .. } => {
                if self
                    .message
                    .as_ref()
                    .is_some_and(|(_, prefix)| !text.starts_with(prefix))
                {
                    let discarded = self.message.as_ref().map(|(id, _)| id.clone());
                    self.text_end(&mut out);
                    out.push(WireEvent::Custom {
                        name: "namzu.provisional_text_discarded".into(),
                        value: json!({"messageId":discarded}),
                    });
                }
                if !text.is_empty() {
                    self.text_start(&mut out);
                    let (id, prefix) = self.message.as_mut().unwrap();
                    if text.len() > 4 * 1024 * 1024 {
                        return Err(bad());
                    }
                    let rest = &text[prefix.len()..];
                    if !rest.is_empty() {
                        out.push(WireEvent::TextContent {
                            message_id: id.clone(),
                            delta: rest.into(),
                        });
                    }
                }
                self.text_end(&mut out);
                self.step_end(&mut out);
            }
            Event::ToolStarted { id, .. } => {
                if !self.tools.contains_key(id) {
                    return Err(bad());
                }
            }
            Event::ToolFinished { id, result, .. } => {
                out.extend(self.tool_result(id, result)?);
            }
            // Host finish() supplies the real settled outcome; raw error strings are omitted.
            Event::TurnStarted
            | Event::ProviderFinished { .. }
            | Event::TurnCompleted { .. }
            | Event::TurnFailed { .. } => {}
        }
        self.account(out)
    }
    pub fn propose(&mut self, call: &ToolCall) -> Result<Vec<WireEvent>, AgentError> {
        if self.terminal
            || !self.started
            || !valid_id(&call.id)
            || call.name.is_empty()
            || call.name.len() > 128
            || call.name.chars().any(char::is_control)
            || !call.arguments.is_object()
        {
            return Err(bad());
        }
        let args = serde_json::to_string(&call.arguments).map_err(|_| bad())?;
        if args.len() > 64 * 1024 {
            return Err(bad());
        }
        let fingerprint: [u8; 32] =
            Sha256::digest(serde_json::to_vec(call).map_err(|_| bad())?).into();
        if let Some(old) = self.tools.get(&call.id) {
            return if old.fingerprint == fingerprint {
                Ok(vec![])
            } else {
                Err(bad())
            };
        }
        if self.tools.len() >= 512 {
            return Err(bad());
        }
        let events = self.account(vec![
            WireEvent::ToolStart {
                tool_call_id: call.id.clone(),
                tool_call_name: call.name.clone(),
            },
            WireEvent::ToolArgs {
                tool_call_id: call.id.clone(),
                delta: args,
            },
            WireEvent::ToolEnd {
                tool_call_id: call.id.clone(),
            },
        ])?;
        self.tools.insert(
            call.id.clone(),
            Tool {
                fingerprint,
                result: false,
            },
        );
        Ok(events)
    }
    fn tool_result(&mut self, id: &str, result: &ToolResult) -> Result<Vec<WireEvent>, AgentError> {
        let Some(state) = self.tools.get_mut(id) else {
            return Err(bad());
        };
        if state.result {
            return Err(bad());
        }
        state.result = true;
        if result.content.len() > 128 * 1024 {
            return Err(bad());
        }
        let mut out = vec![WireEvent::ToolResult {
            message_id: format!("r-{}-{}", self.run, id),
            tool_call_id: id.into(),
            role: "tool".into(),
            content: result.content.clone(),
            metadata: json!({"namzu":{"isError":result.is_error}}),
        }];
        if let Some(image) = &result.image {
            out.push(WireEvent::Custom {
                name: "namzu.image_available".into(),
                value: json!({"toolCallId":id,"width":image.width,"height":image.height}),
            });
        }
        Ok(out)
    }
    pub fn approval(&mut self, id: &str, call: &ToolCall) -> Result<Vec<WireEvent>, AgentError> {
        if !self.started
            || self.terminal
            || !valid_id(id)
            || !valid_id(&call.id)
            || call.name.len() > 128
            || !call.arguments.is_object()
            || serde_json::to_vec(&call.arguments)
                .map_err(|_| bad())?
                .len()
                > 64 * 1024
        {
            return Err(bad());
        }
        self.account(vec![WireEvent::Custom{name:"namzu.approval_required".into(),value:json!({"threadId":self.thread,"runId":self.run,"approvalId":id,"toolCallId":call.id,"name":call.name,"arguments":call.arguments})}])
    }
    pub fn finish(
        &mut self,
        result: &Result<Turn, AgentError>,
        session: Option<&str>,
        telemetry: Option<&str>,
    ) -> Vec<WireEvent> {
        if self.terminal {
            return vec![];
        }
        let mut out = vec![];
        if !self.started {
            self.started = true;
            out.push(WireEvent::RunStarted {
                thread_id: self.thread.clone(),
                run_id: self.run.clone(),
            });
        }
        // Failed emission may have mutated a provisional part whose START never
        // reached the consumer. RUN_ERROR terminates those parts without orphan ENDs.
        if result.is_ok() {
            self.text_end(&mut out);
            self.step_end(&mut out);
        } else {
            self.message = None;
            self.step = None;
        }
        self.terminal = true;
        out.push(match result{
            Ok(turn)=>WireEvent::RunFinished{thread_id:self.thread.clone(),run_id:self.run.clone(),result:json!({"output":turn.output,"stats":turn.stats,"sessionId":session,"telemetryId":telemetry})},
            Err(error)=>{let code=match error{AgentError::Cancelled=>"NAMZU_CANCELLED",AgentError::Timeout=>"NAMZU_TIMEOUT",AgentError::StepLimit=>"NAMZU_STEP_LIMIT",AgentError::ToolLimit=>"NAMZU_TOOL_LIMIT",AgentError::Configuration(_)=>"NAMZU_CONFIGURATION",AgentError::State(_)=>"NAMZU_STATE",AgentError::Protocol(_)=>"NAMZU_PROTOCOL",_=>"NAMZU_PROVIDER"};WireEvent::RunError{code:code.into(),message:"Agent run failed; inspect the private host session before retrying.".into()}},
        });
        if result.is_ok()
            && (self.tools.values().any(|t| !t.result) || out.iter().any(|e| sse(e).is_err()))
        {
            out.pop();
            out.push(WireEvent::RunError {
                code: "NAMZU_OUTPUT_OR_LIFECYCLE_LIMIT".into(),
                message: "The settled result cannot be published on this bounded stream.".into(),
            });
        }
        out
    }
}
#[cfg(test)]
mod tests;
