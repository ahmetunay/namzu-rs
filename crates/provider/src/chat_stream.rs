//! Bounded Chat Completions SSE. Tool arguments remain inert until finish + [DONE].
use namzu_core::{AgentError, Event, EventSink, TurnContext};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{BufRead, Read},
};
pub(crate) fn parse(
    mut reader: impl BufRead,
    ctx: &TurnContext,
    events: &mut dyn EventSink,
    report_cost: bool,
) -> Result<(Value, Option<namzu_core::provider::Cost>), AgentError> {
    let mut total = 0;
    let mut data = String::new();
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut reasoning_content = String::new();
    let mut details: BTreeMap<u64, Value> = BTreeMap::new();
    let mut calls: BTreeMap<u64, Value> = BTreeMap::new();
    let mut finish: Option<String> = None;
    let mut usage = Value::Null;
    let mut reported_cost = None;
    loop {
        ctx.remaining()?;
        let mut line = vec![];
        let n = Read::take(&mut reader, 1024 * 1024 + 1)
            .read_until(b'\n', &mut line)
            .map_err(|_| crate::http::read_error(ctx))?;
        total += n;
        if n == 0 {
            return Err(AgentError::Protocol("Chat SSE ended before DONE"));
        }
        if n > 1024 * 1024 || total > 16 * 1024 * 1024 {
            return Err(AgentError::Protocol("Chat SSE exceeds limit"));
        }
        let line = std::str::from_utf8(&line)
            .map_err(|_| AgentError::Protocol("invalid Chat SSE UTF-8"))?
            .trim_end_matches(['\r', '\n']);
        if let Some(part) = line.strip_prefix("data:") {
            data.push_str(part.strip_prefix(' ').unwrap_or(part));
            data.push('\n');
            if data.len() > 1024 * 1024 {
                return Err(AgentError::Protocol("Chat SSE event exceeds limit"));
            }
            continue;
        }
        if !line.is_empty() || data.is_empty() {
            continue;
        }
        if data.trim() == "[DONE]" {
            let finish = finish.ok_or(AgentError::Protocol("Chat SSE missing finish reason"))?;
            let mut message = json!({"role":"assistant","content":text});
            if !calls.is_empty() {
                let mut values = vec![];
                for (expected, (index, call)) in calls.into_iter().enumerate() {
                    if index != expected as u64 {
                        return Err(AgentError::Protocol("noncontiguous Chat tool indexes"));
                    }
                    values.push(call);
                }
                message["tool_calls"] = json!(values);
            }
            if !reasoning.is_empty() {
                message["reasoning"] = reasoning.into();
            }
            if !reasoning_content.is_empty() {
                message["reasoning_content"] = reasoning_content.into();
            }
            if !details.is_empty() {
                message["reasoning_details"] = details.into_values().collect::<Vec<_>>().into();
            }
            return Ok((
                json!({"choices":[{"finish_reason":finish,"message":message}],"usage":usage}),
                reported_cost,
            ));
        }
        if report_cost && let Some(cost) = crate::services::raw_cost(data.as_bytes())? {
            if reported_cost
                .as_ref()
                .is_some_and(|previous| previous != &cost)
            {
                return Err(AgentError::Protocol("conflicting reported cost chunks"));
            }
            reported_cost = Some(cost);
        }
        let value: Value = serde_json::from_str(data.trim_end())
            .map_err(|_| AgentError::Protocol("invalid Chat SSE JSON"))?;
        data.clear();
        if let Some(error) = crate::services::wire_error(&value) {
            return Err(error);
        }
        if let Some(value) = value.get("usage").filter(|v| !v.is_null()) {
            if !usage.is_null() && usage != *value {
                return Err(AgentError::Protocol("conflicting Chat usage chunks"));
            }
            usage = value.clone();
        }
        let choices = value
            .get("choices")
            .and_then(Value::as_array)
            .ok_or(AgentError::Protocol("Chat SSE choices missing"))?;
        if choices.is_empty() {
            continue;
        }
        if choices.len() != 1 {
            return Err(AgentError::Protocol("Chat SSE requires one choice"));
        }
        let choice = &choices[0];
        if choice.get("index").is_some_and(|v| v.as_u64() != Some(0)) {
            return Err(AgentError::Protocol("invalid Chat choice index"));
        }
        if finish.is_some() {
            return Err(AgentError::Protocol("Chat data after finish"));
        }
        let delta = choice
            .get("delta")
            .ok_or(AgentError::Protocol("Chat delta missing"))?;
        if !delta.is_object() {
            return Err(AgentError::Protocol("Chat delta must be an object"));
        }
        if delta.get("role").is_some_and(|v| v != "assistant") {
            return Err(AgentError::Protocol("invalid Chat delta role"));
        }
        if delta
            .get("refusal")
            .is_some_and(|v| !v.is_null() && v != "")
        {
            return Err(AgentError::Remote(namzu_core::ProviderFailure {
                kind: namzu_core::provider::FailureKind::Rejected,
                http_status: None,
                retry_after_seconds: None,
            }));
        }
        if let Some(v) = delta.get("content").filter(|v| !v.is_null()) {
            let s = v
                .as_str()
                .ok_or(AgentError::Protocol("invalid Chat text delta"))?;
            text.push_str(s);
            if text.len() > 4 * 1024 * 1024 {
                return Err(AgentError::Protocol("Chat text exceeds limit"));
            }
            events.emit(Event::AssistantDelta { text: s.into() })?;
        }
        for key in ["reasoning_content", "reasoning"] {
            if let Some(v) = delta.get(key).filter(|v| !v.is_null()) {
                let target = if key == "reasoning" {
                    &mut reasoning
                } else {
                    &mut reasoning_content
                };
                target.push_str(
                    v.as_str()
                        .ok_or(AgentError::Protocol("invalid Chat reasoning delta"))?,
                );
            }
        }
        if let Some(v) = delta.get("reasoning_details").filter(|v| !v.is_null()) {
            for item in v
                .as_array()
                .ok_or(AgentError::Protocol("invalid reasoning details"))?
            {
                let index = item
                    .get("index")
                    .and_then(Value::as_u64)
                    .unwrap_or(details.len() as u64);
                if index >= 1024 || !item.is_object() {
                    return Err(AgentError::Protocol("invalid reasoning detail index"));
                }
                let current = details.entry(index).or_insert_with(|| json!({}));
                for (key, value) in item.as_object().unwrap() {
                    if matches!(key.as_str(), "text" | "summary" | "data" | "signature") {
                        append(current, key, value)?;
                    } else if current.get(key).is_some_and(|v| v != value) {
                        return Err(AgentError::Protocol("conflicting reasoning detail"));
                    } else {
                        current[key] = value.clone();
                    }
                }
            }
        }
        if let Some(v) = delta.get("tool_calls").filter(|v| !v.is_null()) {
            for fragment in v
                .as_array()
                .ok_or(AgentError::Protocol("invalid Chat tool fragments"))?
            {
                let index = fragment["index"]
                    .as_u64()
                    .filter(|n| *n < 1024)
                    .ok_or(AgentError::Protocol("invalid Chat tool index"))?;
                if fragment.get("type").is_some_and(|v| v != "function") {
                    return Err(AgentError::Protocol("unsupported streamed tool type"));
                }
                let call = calls.entry(index).or_insert_with(
                    || json!({"id":"","type":"function","function":{"name":"","arguments":""}}),
                );
                if let Some(id) = fragment.get("id").filter(|v| !v.is_null()) {
                    append(call, "id", id)?;
                }
                if let Some(function) = fragment.get("function") {
                    if !function.is_object() {
                        return Err(AgentError::Protocol("invalid tool function fragment"));
                    }
                    for key in ["name", "arguments"] {
                        if let Some(value) = function.get(key).filter(|v| !v.is_null()) {
                            append(&mut call["function"], key, value)?;
                        }
                    }
                }
            }
        }
        if let Some(reason) = choice.get("finish_reason").filter(|v| !v.is_null()) {
            finish = Some(
                reason
                    .as_str()
                    .ok_or(AgentError::Protocol("invalid Chat finish reason"))?
                    .into(),
            );
        }
    }
}
fn append(value: &mut Value, key: &str, part: &Value) -> Result<(), AgentError> {
    let part = part
        .as_str()
        .ok_or(AgentError::Protocol("invalid streamed string fragment"))?;
    let mut text = value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    text.push_str(part);
    if text.len() > 1024 * 1024 {
        return Err(AgentError::Protocol("streamed field exceeds limit"));
    }
    value[key] = text.into();
    Ok(())
}
