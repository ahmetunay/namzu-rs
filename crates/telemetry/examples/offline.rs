//! Run without model credentials; emits only the safe trace, never task content.
use namzu_core::{
    AgentError, Cancellation, Completion, Limits, Message, NoopObserver, ScriptedProvider,
    ToolCall, ToolDefinition, ToolResult, Tools, TurnContext, run_history,
};
use namzu_telemetry::{Observed, Recorder, Source};
struct NoTools;
impl Tools for NoTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![]
    }
    fn call(&mut self, _: &ToolCall, _: &TurnContext) -> Result<ToolResult, AgentError> {
        Err(AgentError::Configuration("no tools in this example"))
    }
}
fn main() -> Result<(), AgentError> {
    let mut recorder = Recorder::new(Source::Sdk, 64)?;
    let result = run_history(
        &mut ScriptedProvider(
            [Completion {
                text: "private answer".into(),
                tool_calls: vec![],
            }]
            .into(),
        ),
        &mut NoTools,
        vec![Message::User {
            content: "private prompt".into(),
        }],
        Limits::default(),
        Cancellation::default(),
        &mut Observed {
            inner: &mut NoopObserver,
            recorder: Some(&mut recorder),
        },
    );
    recorder.finish(result.as_ref().err());
    result?;
    recorder.snapshot().validate()?;
    println!(
        "{}",
        serde_json::to_string(recorder.snapshot())
            .map_err(|_| AgentError::State("trace encoding"))?
    );
    Ok(())
}
