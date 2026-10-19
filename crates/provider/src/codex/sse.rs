use namzu_core::{AgentError, Completion, Event, EventSink, ToolCall, TurnContext};
use serde_json::Value;
use std::{collections::BTreeMap, io::BufRead};

#[cfg(test)]
pub(super) fn parse(
    reader: impl BufRead,
    context: &TurnContext,
) -> Result<(Completion, Vec<Value>), AgentError> {
    parse_stream(reader, context, &mut namzu_core::NoopEvents)
}

#[cfg(test)]
pub(super) fn parse_stream(
    reader: impl BufRead,
    context: &TurnContext,
    events: &mut dyn EventSink,
) -> Result<(Completion, Vec<Value>), AgentError> {
    let (completion, items, _) = parse_stream_metadata(reader, context, events)?;
    Ok((completion, items))
}
pub(super) fn parse_stream_metadata(
    mut reader: impl BufRead,
    context: &TurnContext,
    events: &mut dyn EventSink,
) -> Result<(Completion, Vec<Value>, namzu_core::ResponseMetadata), AgentError> {
    let mut total = 0;
    let mut data = String::new();
    let mut done = BTreeMap::new();
    loop {
        context.remaining()?;
        let mut line = vec![];
        let count = std::io::Read::take(&mut reader, 1024 * 1024 + 1)
            .read_until(b'\n', &mut line)
            .map_err(|_| crate::http::read_error(context))?;
        if count == 0 {
            return Err(AgentError::Protocol(
                "Codex stream ended without completed response",
            ));
        }
        total += count;
        if count > 1024 * 1024 || total > 16 * 1024 * 1024 {
            return Err(AgentError::Protocol("Codex stream exceeds limit"));
        }
        let line = std::str::from_utf8(&line)
            .map_err(|_| AgentError::Protocol("invalid SSE UTF-8"))?
            .trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            if data.is_empty() {
                continue;
            }
            if data.trim() == "[DONE]" {
                return Err(AgentError::Protocol("missing completed Codex event"));
            }
            let event: Value = serde_json::from_str(data.trim_end())
                .map_err(|_| AgentError::Protocol("invalid Codex SSE JSON"))?;
            data.clear();
            match event["type"].as_str() {
                Some("response.output_text.delta") => {
                    let text = event["delta"]
                        .as_str()
                        .ok_or(AgentError::Protocol("invalid text delta"))?;
                    events.emit(Event::AssistantDelta { text: text.into() })?;
                    context.remaining()?;
                }
                Some("response.output_item.done") => {
                    let index = event["output_index"]
                        .as_u64()
                        .filter(|index| *index < 4096)
                        .ok_or(AgentError::Protocol("invalid output index"))?;
                    if !event["item"].is_object()
                        || done.insert(index, event["item"].clone()).is_some()
                    {
                        return Err(AgentError::Protocol("duplicate or invalid finalized item"));
                    }
                }
                Some("response.completed") => {
                    if event["response"]["status"] != "completed" {
                        return Err(AgentError::Protocol("Codex response was not complete"));
                    }
                    let output = event["response"]["output"]
                        .as_array()
                        .ok_or(AgentError::Protocol("missing Codex output"))?;
                    let items = if output.is_empty() {
                        done.into_values().collect()
                    } else {
                        output.clone()
                    };
                    if items.len() > 4096 {
                        return Err(AgentError::Protocol("too many Codex output items"));
                    }
                    let completion = decode(&items)?;
                    namzu_core::provider::validate_completion(&completion)?;
                    let metadata = crate::contract::metadata(&event["response"], true)?;
                    return Ok((completion, items, metadata));
                }
                Some("response.incomplete" | "response.failed" | "error") => {
                    return Err(AgentError::Remote(namzu_core::ProviderFailure {
                        kind: namzu_core::provider::FailureKind::Incomplete,
                        http_status: None,
                        retry_after_seconds: None,
                    }));
                }
                _ => (),
            }
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push_str(value.strip_prefix(' ').unwrap_or(value));
            data.push('\n');
            if data.len() > 1024 * 1024 {
                return Err(AgentError::Protocol("Codex SSE event exceeds limit"));
            }
        }
    }
}
fn decode(items: &[Value]) -> Result<Completion, AgentError> {
    let mut completion = Completion::default();
    let mut final_text = String::new();
    let mut phased = false;
    for item in items {
        match item["type"].as_str() {
            Some("reasoning") => (),
            Some("message") => {
                let parts = item["content"]
                    .as_array()
                    .ok_or(AgentError::Protocol("invalid output message"))?;
                let mut text = String::new();
                for part in parts {
                    match part["type"].as_str() {
                        Some("output_text") => text.push_str(
                            part["text"]
                                .as_str()
                                .ok_or(AgentError::Protocol("invalid output text"))?,
                        ),
                        Some("refusal") => {
                            return Err(AgentError::Provider("Codex response refused".into()));
                        }
                        _ => return Err(AgentError::Protocol("unsupported Codex content")),
                    }
                }
                completion.text.push_str(&text);
                if item["phase"] == "final_answer" {
                    final_text.push_str(&text);
                    phased = true;
                }
            }
            Some("function_call") => {
                let field = |name: &str| {
                    item[name]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .ok_or(AgentError::Protocol("invalid Codex function call"))
                };
                let arguments: Value = serde_json::from_str(field("arguments")?)
                    .map_err(|_| AgentError::Protocol("invalid Codex function arguments"))?;
                if !arguments.is_object() {
                    return Err(AgentError::Protocol("Codex arguments must be an object"));
                }
                completion.tool_calls.push(ToolCall {
                    id: field("call_id")?.into(),
                    name: field("name")?.into(),
                    arguments,
                });
            }
            _ => return Err(AgentError::Protocol("unsupported Codex output item")),
        }
    }
    if phased {
        completion.text = final_text;
    }
    if completion.text.is_empty() && completion.tool_calls.is_empty() {
        return Err(AgentError::Protocol("empty Codex completion"));
    }
    Ok(completion)
}
