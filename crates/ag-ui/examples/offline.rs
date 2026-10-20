//! Scripted native turn -> AG-UI SSE -> incremental decoder/client. No account/network.
use namzu_ag_ui::{
    Mapper, WireEvent,
    client::{ClientState, Decoder},
};
use namzu_core::{
    AgentError, Cancellation, Completion, Event, Limits, Message, Observer, ScriptedProvider,
    Stats, ToolCall, ToolDefinition, ToolResult, Tools, TurnContext, run_history,
};
use serde_json::json;
struct Echo;
impl Tools for Echo {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "echo".into(),
            description: "offline fixture".into(),
            parameters: json!({"type":"object"}),
        }]
    }
    fn call(&mut self, c: &ToolCall, _: &TurnContext) -> Result<ToolResult, AgentError> {
        Ok(ToolResult::ok(c.arguments["value"].to_string()))
    }
}
struct UI {
    mapper: Mapper,
    events: Vec<WireEvent>,
}
impl Observer for UI {
    fn checkpoint(
        &mut self,
        _: &[Message],
        _: &Stats,
        pending: Option<&ToolCall>,
    ) -> Result<(), AgentError> {
        if let Some(c) = pending {
            self.events.extend(self.mapper.propose(c)?);
        }
        Ok(())
    }
    fn event(&mut self, e: Event) -> Result<(), AgentError> {
        self.events.extend(self.mapper.map(&e)?);
        Ok(())
    }
}
fn main() -> Result<(), AgentError> {
    let mut ui = UI {
        mapper: Mapper::new("offline-thread", "offline-run")?,
        events: vec![],
    };
    let mut provider = ScriptedProvider(
        [
            Completion {
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call".into(),
                    name: "echo".into(),
                    arguments: json!({"value":42}),
                }],
            },
            Completion {
                text: "42".into(),
                tool_calls: vec![],
            },
        ]
        .into(),
    );
    let result = run_history(
        &mut provider,
        &mut Echo,
        vec![Message::User {
            content: "Return 42".into(),
        }],
        Limits::default(),
        Cancellation::default(),
        &mut ui,
    );
    ui.events.extend(ui.mapper.finish(&result, None, None));
    result?;
    let mut decoder = Decoder::default();
    let mut client = ClientState::default();
    let mut bytes = vec![];
    for event in ui.events {
        bytes.extend(namzu_ag_ui::sse(&event)?);
    }
    for chunk in bytes.chunks(7) {
        for event in decoder.feed(chunk)? {
            client.accept(&event)?;
        }
    }
    decoder.eof()?;
    client.eof()?;
    if client.output.as_deref() != Some("42") {
        return Err(AgentError::Protocol("offline client oracle failed"));
    }
    use std::io::Write;
    std::io::stdout()
        .write_all(&bytes)
        .map_err(|_| AgentError::State("client output closed"))
}
