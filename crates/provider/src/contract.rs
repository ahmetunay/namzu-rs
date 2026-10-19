//! Shared wire usage/error normalization. Never consumes server error bodies.
use namzu_core::{AgentError, ResponseMetadata, TokenUsage};
use serde_json::Value;
fn count(object: &Value, key: &str) -> Result<Option<u64>, AgentError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or(AgentError::Protocol("invalid provider usage counter")),
    }
}
fn detail(object: &Value, parent: &str, key: &str) -> Result<Option<u64>, AgentError> {
    match object.get(parent) {
        None | Some(Value::Null) => Ok(None),
        Some(v) if v.is_object() => count(v, key),
        _ => Err(AgentError::Protocol("invalid provider usage details")),
    }
}
pub(crate) fn metadata(value: &Value, responses: bool) -> Result<ResponseMetadata, AgentError> {
    let Some(usage) = value.get("usage").filter(|v| !v.is_null()) else {
        return Ok(ResponseMetadata::default());
    };
    if !usage.is_object() {
        return Err(AgentError::Protocol("invalid provider usage"));
    }
    let (input, output) = if responses {
        ("input_tokens", "output_tokens")
    } else {
        ("prompt_tokens", "completion_tokens")
    };
    let tokens = TokenUsage {
        input_tokens: count(usage, input)?,
        output_tokens: count(usage, output)?,
        total_tokens: count(usage, "total_tokens")?,
        cached_input_tokens: detail(
            usage,
            if responses {
                "input_tokens_details"
            } else {
                "prompt_tokens_details"
            },
            "cached_tokens",
        )?,
        reasoning_tokens: detail(
            usage,
            if responses {
                "output_tokens_details"
            } else {
                "completion_tokens_details"
            },
            "reasoning_tokens",
        )?,
    };
    tokens.validate()?;
    Ok(ResponseMetadata {
        usage: Some(tokens),
        cost: None,
    })
}
pub(crate) fn status(response: &crate::http::Response) -> AgentError {
    let retry_after = response
        .header("retry-after")
        .and_then(|s| s.parse::<u32>().ok());
    AgentError::Remote(namzu_core::ProviderFailure::http(
        response.status().as_u16(),
        retry_after,
    ))
}
