//! Incremental SSE decoder and bounded client lifecycle. No networking or permissions.
use crate::{MAX_FRAME, MAX_STREAM, WireEvent, bad};
use namzu_core::AgentError;
use serde_json::Value;
use std::collections::HashMap;
#[derive(Default)]
pub struct Decoder {
    line: Vec<u8>,
    data: Vec<u8>,
    bytes: usize,
}
impl Decoder {
    pub fn feed(&mut self, input: &[u8]) -> Result<Vec<WireEvent>, AgentError> {
        self.bytes = self
            .bytes
            .checked_add(input.len())
            .filter(|n| *n <= MAX_STREAM)
            .ok_or_else(bad)?;
        let mut events = vec![];
        for b in input {
            if *b == b'\n' {
                let line = self.line.strip_suffix(b"\r").unwrap_or(&self.line);
                if line.is_empty() {
                    if !self.data.is_empty() {
                        let e =
                            serde_json::from_slice::<WireEvent>(&self.data).map_err(|_| bad())?;
                        events.push(e);
                        self.data.clear();
                    }
                } else if let Some(v) = line.strip_prefix(b"data:") {
                    if !self.data.is_empty() {
                        self.data.push(b'\n');
                    }
                    self.data.extend(v.strip_prefix(b" ").unwrap_or(v));
                } else if !line.starts_with(b":")
                    && !line.starts_with(b"event:")
                    && !line.starts_with(b"id:")
                    && !line.starts_with(b"retry:")
                {
                    return Err(bad());
                }
                self.line.clear();
            } else {
                self.line.push(*b);
            }
            if self.line.len() > MAX_FRAME || self.data.len() > MAX_FRAME {
                return Err(bad());
            }
        }
        Ok(events)
    }
    pub fn eof(&self) -> Result<(), AgentError> {
        if !self.line.is_empty() || !self.data.is_empty() {
            Err(bad())
        } else {
            Ok(())
        }
    }
}
#[derive(Default)]
pub struct ClientState {
    bytes: usize,
    step: Option<String>,
    pub output: Option<String>,
    pub error_code: Option<String>,
    pub finished: bool,
    thread: Option<String>,
    run: Option<String>,
    messages: HashMap<String, bool>,
    tools: HashMap<String, u8>,
    pub result: Option<Value>,
}
impl ClientState {
    pub fn accept(&mut self, event: &WireEvent) -> Result<(), AgentError> {
        if self.finished {
            return Err(bad());
        }
        let size = serde_json::to_vec(event).map_err(|_| bad())?.len();
        if size > MAX_FRAME {
            return Err(bad());
        }
        self.bytes = self
            .bytes
            .checked_add(size)
            .filter(|v| *v <= MAX_STREAM)
            .ok_or_else(bad)?;
        match event {
            WireEvent::RunStarted { thread_id, run_id } => {
                if self.run.is_some() {
                    return Err(bad());
                }
                self.thread = Some(thread_id.clone());
                self.run = Some(run_id.clone());
            }
            _ if self.run.is_none() => return Err(bad()),
            WireEvent::StepStarted { step_name } => {
                if self.step.is_some() {
                    return Err(bad());
                }
                self.step = Some(step_name.clone());
            }
            WireEvent::StepFinished { step_name } => {
                if self.step.as_ref() != Some(step_name) {
                    return Err(bad());
                }
                self.step = None;
            }
            WireEvent::TextStart { message_id, .. } => {
                if self.messages.len() >= 1024
                    || self.messages.insert(message_id.clone(), false).is_some()
                {
                    return Err(bad());
                }
            }
            WireEvent::TextContent { message_id, .. } => {
                if self.messages.get(message_id) != Some(&false) {
                    return Err(bad());
                }
            }
            WireEvent::TextEnd { message_id } => {
                if self.messages.insert(message_id.clone(), true) != Some(false) {
                    return Err(bad());
                }
            }
            WireEvent::ToolStart { tool_call_id, .. } => {
                if self.tools.len() >= 512 || self.tools.insert(tool_call_id.clone(), 0).is_some() {
                    return Err(bad());
                }
            }
            WireEvent::ToolArgs { tool_call_id, .. } => {
                if self.tools.get(tool_call_id) != Some(&0) {
                    return Err(bad());
                }
            }
            WireEvent::ToolEnd { tool_call_id } => {
                if self.tools.insert(tool_call_id.clone(), 1) != Some(0) {
                    return Err(bad());
                }
            }
            WireEvent::ToolResult { tool_call_id, .. } => {
                if self.tools.insert(tool_call_id.clone(), 2) != Some(1) {
                    return Err(bad());
                }
            }
            WireEvent::RunFinished {
                thread_id,
                run_id,
                result,
            } => {
                if self.step.is_some()
                    || self.thread.as_ref() != Some(thread_id)
                    || self.run.as_ref() != Some(run_id)
                    || self.messages.values().any(|v| !*v)
                    || self.tools.values().any(|v| *v != 2)
                {
                    return Err(bad());
                }
                self.output = Some(
                    result["output"]
                        .as_str()
                        .filter(|s| s.len() <= 4 * 1024 * 1024)
                        .ok_or_else(bad)?
                        .into(),
                );
                self.result = Some(result.clone());
                self.finished = true;
            }
            WireEvent::RunError { code, .. } => {
                self.error_code = Some(code.clone());
                self.finished = true;
            }
            _ => {}
        }
        Ok(())
    }
    pub fn eof(&self) -> Result<(), AgentError> {
        if self.finished { Ok(()) } else { Err(bad()) }
    }
}
