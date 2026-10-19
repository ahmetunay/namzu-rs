//! Versioned provider contract. No transport, prices, credentials or automatic retry policy.
use crate::{AgentError, Completion, Message, ToolDefinition, TurnContext};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Support {
    Supported,
    Unsupported,
    #[default]
    Unknown,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Streaming {
    #[default]
    Buffered,
    Incremental,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ProviderCapabilities {
    pub tools: Support,
    pub images: Support,
    pub replay: Support,
    pub usage: Support,
    pub streaming: Streaming,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderInfo {
    pub contract_version: u32,
    pub provider: String,
    pub model: Option<String>,
    pub capabilities: ProviderCapabilities,
}
impl Default for ProviderInfo {
    fn default() -> Self {
        Self {
            contract_version: 1,
            provider: "custom".into(),
            model: None,
            capabilities: Default::default(),
        }
    }
}
impl ProviderInfo {
    pub fn validate(&self) -> Result<(), AgentError> {
        if self.contract_version != 1
            || self.provider.is_empty()
            || self.provider.len() > 128
            || !self
                .provider
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            || self
                .model
                .as_ref()
                .is_some_and(|s| s.is_empty() || s.len() > 256 || s.chars().any(char::is_control))
        {
            return Err(AgentError::Configuration(
                "invalid provider contract descriptor",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct TokenUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    /// Cache reads are included in input, reasoning is included in output.
    pub cached_input_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
}
impl TokenUsage {
    pub fn validate(&self) -> Result<(), AgentError> {
        if self
            .cached_input_tokens
            .zip(self.input_tokens)
            .is_some_and(|(a, b)| a > b)
            || self
                .reasoning_tokens
                .zip(self.output_tokens)
                .is_some_and(|(a, b)| a > b)
            || self
                .input_tokens
                .zip(self.output_tokens)
                .zip(self.total_tokens)
                .is_some_and(|((a, b), t)| a.checked_add(b) != Some(t))
            || self.total_tokens.is_some_and(|t| {
                self.input_tokens.is_some_and(|v| v > t)
                    || self.output_tokens.is_some_and(|v| v > t)
            })
        {
            return Err(AgentError::Protocol("inconsistent provider token usage"));
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CostSource {
    Reported,
    Estimated,
}
/// Fixed point USD (1 dollar = 1_000_000_000 nano_usd). None means unknown, never free.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Cost {
    pub nano_usd: u64,
    pub source: CostSource,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Pricing {
    pub input_nano_usd_per_token: u64,
    pub output_nano_usd_per_token: u64,
    pub cached_input_nano_usd_per_token: Option<u64>,
}
impl Pricing {
    pub fn estimate(&self, usage: &TokenUsage) -> Result<Option<Cost>, AgentError> {
        usage.validate()?;
        let Some((input, output)) = usage.input_tokens.zip(usage.output_tokens) else {
            return Ok(None);
        };
        let (input, cached) = if let Some(rate) = self.cached_input_nano_usd_per_token {
            let Some(cached) = usage.cached_input_tokens else {
                return Ok(None);
            };
            (
                input
                    .checked_sub(cached)
                    .ok_or(AgentError::Protocol("invalid cache count"))?,
                cached.checked_mul(rate),
            )
        } else {
            (input, Some(0))
        };
        let amount = input
            .checked_mul(self.input_nano_usd_per_token)
            .and_then(|a| {
                output
                    .checked_mul(self.output_nano_usd_per_token)
                    .and_then(|b| a.checked_add(b))
            })
            .and_then(|a| cached.and_then(|b| a.checked_add(b)))
            .ok_or(AgentError::Protocol("provider cost overflow"))?;
        Ok(Some(Cost {
            nano_usd: amount,
            source: CostSource::Estimated,
        }))
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ResponseMetadata {
    pub usage: Option<TokenUsage>,
    pub cost: Option<Cost>,
}
impl ResponseMetadata {
    pub fn validate(&self) -> Result<(), AgentError> {
        if let Some(u) = &self.usage {
            u.validate()?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReportedTotal {
    pub sum: u64,
    pub samples: usize,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct UsageTotals {
    pub responses: usize,
    pub input_tokens: Option<ReportedTotal>,
    pub output_tokens: Option<ReportedTotal>,
    pub total_tokens: Option<ReportedTotal>,
    pub cached_input_tokens: Option<ReportedTotal>,
    pub reasoning_tokens: Option<ReportedTotal>,
    pub reported_cost_nano_usd: Option<ReportedTotal>,
    pub estimated_cost_nano_usd: Option<ReportedTotal>,
}
impl UsageTotals {
    pub fn is_empty(&self) -> bool {
        self.input_tokens.is_none()
            && self.output_tokens.is_none()
            && self.total_tokens.is_none()
            && self.cached_input_tokens.is_none()
            && self.reasoning_tokens.is_none()
            && self.reported_cost_nano_usd.is_none()
            && self.estimated_cost_nano_usd.is_none()
    }
    pub fn record(&mut self, metadata: &ResponseMetadata) -> Result<(), AgentError> {
        metadata.validate()?;
        let mut next = self.clone();
        next.responses = next
            .responses
            .checked_add(1)
            .ok_or(AgentError::Protocol("provider usage overflow"))?;
        fn add(target: &mut Option<ReportedTotal>, value: Option<u64>) -> Result<(), AgentError> {
            if let Some(value) = value {
                let t = target.get_or_insert(ReportedTotal { sum: 0, samples: 0 });
                t.sum = t
                    .sum
                    .checked_add(value)
                    .ok_or(AgentError::Protocol("provider usage overflow"))?;
                t.samples = t
                    .samples
                    .checked_add(1)
                    .ok_or(AgentError::Protocol("provider usage overflow"))?;
            }
            Ok(())
        }
        if let Some(u) = &metadata.usage {
            add(&mut next.input_tokens, u.input_tokens)?;
            add(&mut next.output_tokens, u.output_tokens)?;
            add(&mut next.total_tokens, u.total_tokens)?;
            add(&mut next.cached_input_tokens, u.cached_input_tokens)?;
            add(&mut next.reasoning_tokens, u.reasoning_tokens)?;
        }
        if let Some(c) = &metadata.cost {
            match c.source {
                CostSource::Reported => add(&mut next.reported_cost_nano_usd, Some(c.nano_usd))?,
                CostSource::Estimated => add(&mut next.estimated_cost_nano_usd, Some(c.nano_usd))?,
            }
        }
        *self = next;
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    Authentication,
    Permission,
    RateLimited,
    NotFound,
    InvalidRequest,
    Unavailable,
    Transport,
    Incomplete,
    Rejected,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderFailure {
    pub kind: FailureKind,
    pub http_status: Option<u16>,
    pub retry_after_seconds: Option<u32>,
}
impl ProviderFailure {
    pub fn http(status: u16, retry_after: Option<u32>) -> Self {
        let kind = match status {
            401 => FailureKind::Authentication,
            403 => FailureKind::Permission,
            404 => FailureKind::NotFound,
            429 => FailureKind::RateLimited,
            500..=599 => FailureKind::Unavailable,
            _ => FailureKind::InvalidRequest,
        };
        Self {
            kind,
            http_status: Some(status),
            retry_after_seconds: retry_after.filter(|n| *n <= 86400),
        }
    }
    pub fn transport() -> Self {
        Self {
            kind: FailureKind::Transport,
            http_status: None,
            retry_after_seconds: None,
        }
    }
}
impl std::fmt::Display for ProviderFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.http_status {
            Some(status) => write!(f, "HTTP status {status}"),
            None => write!(f, "{:?}", self.kind),
        }
    }
}
impl std::error::Error for ProviderFailure {}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}
/// Shared shape/budget validation before transport/auth effects. Conversation replay ownership
/// remains the core history validator and the provider-specific replay codec's responsibility.
pub fn validate_request(
    messages: &[Message],
    tools: &[ToolDefinition],
    info: &ProviderInfo,
    ctx: &TurnContext,
) -> Result<(), AgentError> {
    ctx.remaining()?;
    info.validate()?;
    if messages.len() > 16384 || tools.len() > 1024 {
        return Err(AgentError::Configuration("provider request item limit"));
    }
    if !tools.is_empty() && info.capabilities.tools == Support::Unsupported {
        return Err(AgentError::Configuration(
            "provider transport does not support tools",
        ));
    }
    let mut names = HashSet::new();
    for tool in tools {
        if !valid_name(&tool.name)
            || tool.description.len() > 65536
            || !tool.parameters.is_object()
            || !names.insert(&tool.name)
        {
            return Err(AgentError::Configuration(
                "invalid or duplicate provider tool schema",
            ));
        }
    }
    for message in messages {
        match message {
            Message::ToolImage { image, .. } => {
                if info.capabilities.images == Support::Unsupported {
                    return Err(AgentError::Configuration(
                        "provider transport does not support images",
                    ));
                }
                image.png()?;
            }
            Message::Assistant { tool_calls, .. } => validate_calls(tool_calls)?,
            _ => (),
        }
    }
    // Include wire JSON overhead, schemas and opaque state; never truncate silently.
    let mut budget = Budget(0);
    serde_json::to_writer(&mut budget, &(messages, tools))
        .map_err(|_| AgentError::Configuration("provider request exceeds 4 MiB"))?;
    ctx.remaining()?;
    Ok(())
}
struct Budget(usize);
impl std::io::Write for Budget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        if self.0 > 4 * 1024 * 1024 {
            return Err(std::io::Error::other("request budget"));
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn validate_calls(calls: &[crate::ToolCall]) -> Result<(), AgentError> {
    if calls.len() > 1024 {
        return Err(AgentError::Protocol("provider tool call limit"));
    }
    let mut ids = HashSet::new();
    for c in calls {
        if c.id.is_empty()
            || c.id.len() > 256
            || c.id.chars().any(char::is_control)
            || !ids.insert(&c.id)
            || !valid_name(&c.name)
            || !c.arguments.is_object()
        {
            return Err(AgentError::Protocol("invalid provider tool call"));
        }
    }
    Ok(())
}
pub fn validate_completion(completion: &Completion) -> Result<(), AgentError> {
    if completion.text.len() > 4 * 1024 * 1024 {
        return Err(AgentError::Protocol("provider completion exceeds limit"));
    }
    validate_calls(&completion.tool_calls)
}
