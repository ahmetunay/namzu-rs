//! Explicit compatible-service profiles. No discovery, anonymous gateway impersonation or fallback.
use namzu_core::{
    AgentError,
    provider::{Cost, CostSource, FailureKind, ProviderFailure},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Service {
    #[default]
    OpenaiCompatible,
    Openrouter,
    Zen,
    ZenGo,
    Anthropic,
    Google,
    Deepseek,
    Bedrock,
    Lmstudio,
    Ollama,
}
impl Service {
    pub fn key_optional(self) -> bool {
        matches!(self, Self::Lmstudio | Self::Ollama)
    }
    pub fn normalize_model(self, model: &str) -> &str {
        match self {
            Self::Zen => model.strip_prefix("zen/").unwrap_or(model),
            Self::ZenGo => model.strip_prefix("go/").unwrap_or(model),
            Self::Google => model.strip_prefix("models/").unwrap_or(model),
            _ => model,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::OpenaiCompatible => "openai-compatible",
            Self::Openrouter => "openrouter",
            Self::Zen => "zen",
            Self::ZenGo => "zen-go",
            Self::Anthropic => "anthropic",
            Self::Google => "google",
            Self::Deepseek => "deepseek",
            Self::Bedrock => "bedrock",
            Self::Lmstudio => "lmstudio",
            Self::Ollama => "ollama",
        }
    }
    pub fn parse(name: &str) -> Result<Self, AgentError> {
        match name {
            "openai-compatible" => Ok(Self::OpenaiCompatible),
            "openrouter" => Ok(Self::Openrouter),
            "zen" => Ok(Self::Zen),
            "zen-go" => Ok(Self::ZenGo),
            "anthropic" => Ok(Self::Anthropic),
            "google" => Ok(Self::Google),
            "deepseek" => Ok(Self::Deepseek),
            "bedrock" => Ok(Self::Bedrock),
            "lmstudio" => Ok(Self::Lmstudio),
            "ollama" => Ok(Self::Ollama),
            _ => Err(AgentError::Configuration("unsupported compatible service")),
        }
    }
    pub fn base_url(self) -> &'static str {
        match self {
            Self::OpenaiCompatible => "https://api.openai.com/v1",
            Self::Openrouter => "https://openrouter.ai/api/v1",
            Self::Zen => "https://opencode.ai/zen/v1",
            Self::ZenGo => "https://opencode.ai/zen/go/v1",
            Self::Anthropic => "https://api.anthropic.com/v1",
            Self::Google => "https://generativelanguage.googleapis.com/v1beta",
            Self::Deepseek => "https://api.deepseek.com",
            Self::Bedrock => "https://bedrock-runtime.us-east-1.amazonaws.com",
            Self::Lmstudio => "http://localhost:1234/v1",
            Self::Ollama => "http://localhost:11434",
        }
    }
    pub fn key_env(self) -> &'static str {
        match self {
            Self::OpenaiCompatible => "OPENAI_API_KEY",
            Self::Openrouter => "OPENROUTER_API_KEY",
            Self::Zen | Self::ZenGo => "OPENCODE_API_KEY",
            Self::Anthropic => "ANTHROPIC_API_KEY",
            Self::Google => "GOOGLE_API_KEY",
            Self::Deepseek => "DEEPSEEK_API_KEY",
            Self::Bedrock => "AWS_BEARER_TOKEN_BEDROCK",
            Self::Lmstudio => "LMSTUDIO_API_KEY",
            Self::Ollama => "OLLAMA_API_KEY",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    #[default]
    Auto,
    Chat,
    Responses,
    Messages,
    Google,
    Converse,
    Ollama,
}
impl Format {
    pub fn native(self) -> bool {
        matches!(
            self,
            Self::Messages | Self::Google | Self::Converse | Self::Ollama
        )
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
    pub format: Format,
    pub stream: bool,
    pub reasoning_effort: Option<String>,
    pub max_output_tokens: Option<u32>,
    pub site_url: Option<String>,
    pub site_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aws_region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bedrock_auth: Option<String>,
}
impl Options {
    pub fn validate(&self) -> Result<(), AgentError> {
        if self
            .max_output_tokens
            .is_some_and(|n| n == 0 || n > 1_000_000)
            || self.reasoning_effort.as_ref().is_some_and(|s| {
                !["none", "minimal", "low", "medium", "high", "xhigh"].contains(&s.as_str())
            })
            || self.site_name.as_ref().is_some_and(|s| {
                s.is_empty() || s.len() > 128 || !s.bytes().all(|b| (32..=126).contains(&b))
            })
        {
            return Err(AgentError::Configuration(
                "invalid compatible-service options",
            ));
        }
        if let Some(value) = &self.site_url {
            let u = url::Url::parse(value)
                .map_err(|_| AgentError::Configuration("invalid attribution URL"))?;
            if value.len() > 2048
                || !["https", "http"].contains(&u.scheme())
                || u.host_str().is_none()
                || !u.username().is_empty()
                || u.password().is_some()
                || u.query().is_some()
                || u.fragment().is_some()
            {
                return Err(AgentError::Configuration("invalid attribution URL"));
            }
        }
        Ok(())
    }
    pub fn resolved(&self, service: Service, model: &str) -> Result<Format, AgentError> {
        self.validate()?;
        if service != Service::Openrouter && (self.site_url.is_some() || self.site_name.is_some()) {
            return Err(AgentError::Configuration(
                "attribution headers apply only to OpenRouter",
            ));
        }
        let model = service.normalize_model(model);
        let expected = match service {
            Service::Anthropic => Some(Format::Messages),
            Service::Google => Some(Format::Google),
            Service::Bedrock => Some(Format::Converse),
            Service::Ollama => Some(Format::Ollama),
            Service::Zen | Service::ZenGo if model.starts_with("claude-") => Some(Format::Messages),
            Service::Zen | Service::ZenGo if model.starts_with("gemini-") => Some(Format::Google),
            _ => None,
        };
        let format = match self.format {
            Format::Auto => expected.unwrap_or_else(|| {
                if matches!(service, Service::Zen | Service::ZenGo)
                    && (model == "muse-spark"
                        || model.starts_with("gpt-")
                        || model.as_bytes().first() == Some(&b'o')
                            && model.as_bytes().get(1).is_some_and(u8::is_ascii_digit))
                {
                    Format::Responses
                } else {
                    Format::Chat
                }
            }),
            f => f,
        };
        if expected.is_some_and(|f| f != format)
            || expected.is_none() && format.native()
            || matches!(
                service,
                Service::Openrouter | Service::Deepseek | Service::Lmstudio
            ) && format != Format::Chat
        {
            return Err(AgentError::Configuration(
                "wire format does not match selected service/model",
            ));
        }
        if format.native() && self.stream {
            return Err(AgentError::Configuration(
                "native driver currently supports buffered JSON only; use --api-stream false",
            ));
        }
        if self.reasoning_effort.is_some()
            && (format.native() || matches!(service, Service::Deepseek | Service::Lmstudio))
        {
            return Err(AgentError::Configuration(
                "reasoning effort is not implemented on this wire",
            ));
        }
        if self
            .thinking
            .as_ref()
            .is_some_and(|s| !["adaptive", "enabled", "disabled"].contains(&s.as_str()))
            || self.thinking.is_some() && service != Service::Deepseek
        {
            return Err(AgentError::Configuration(
                "thinking type applies only to DeepSeek Chat",
            ));
        }
        if self
            .bedrock_auth
            .as_ref()
            .is_some_and(|s| !["bearer", "sigv4"].contains(&s.as_str()))
            || (self.bedrock_auth.is_some() || self.aws_region.is_some())
                && service != Service::Bedrock
            || self.bedrock_auth.as_deref() == Some("sigv4") && self.aws_region.is_none()
            || self.aws_region.is_some() && self.bedrock_auth.as_deref() != Some("sigv4")
            || self.aws_region.as_ref().is_some_and(|s| {
                s.is_empty()
                    || s.len() > 64
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            })
        {
            return Err(AgentError::Configuration(
                "invalid Bedrock auth/region options",
            ));
        }
        Ok(format)
    }
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}
/// Decimal USD -> integer nano-USD, rounding positive sub-nano amounts upward.
/// Parse an explicit decimal value; production uses raw_cost to preserve the original JSON lexeme.
fn reported_cost(value: &Value) -> Result<Option<Cost>, AgentError> {
    if value.is_null() {
        return Ok(None);
    }
    let text = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return Err(AgentError::Protocol("invalid reported service cost")),
    };
    fn parse(s: &str) -> Option<u64> {
        if s.is_empty() || s.len() > 96 || s.starts_with(['-', '+']) {
            return None;
        }
        let mut exp_parts = s.split(['e', 'E']);
        let base = exp_parts.next()?;
        let exponent = exp_parts
            .next()
            .map(str::parse::<i32>)
            .transpose()
            .ok()?
            .unwrap_or(0);
        if exp_parts.next().is_some() || !(-128..=128).contains(&exponent) {
            return None;
        }
        let mut parts = base.split('.');
        let whole = parts.next()?;
        let fraction = parts.next().unwrap_or("");
        if parts.next().is_some()
            || whole.is_empty()
            || !whole
                .bytes()
                .chain(fraction.bytes())
                .all(|b| b.is_ascii_digit())
            || base.ends_with('.')
        {
            return None;
        }
        let coefficient = whole
            .bytes()
            .chain(fraction.bytes())
            .try_fold(0u128, |a, b| {
                a.checked_mul(10)?.checked_add(u128::from(b - b'0'))
            })?;
        if coefficient == 0 {
            return Some(0);
        }
        let scale = 9 + exponent - i32::try_from(fraction.len()).ok()?;
        let result = if scale >= 0 {
            coefficient.checked_mul(10u128.checked_pow(scale as u32)?)?
        } else if -scale > 38 {
            1
        } else {
            let divisor = 10u128.checked_pow((-scale) as u32)?;
            coefficient / divisor + u128::from(coefficient % divisor != 0)
        };
        u64::try_from(result).ok()
    }
    Ok(Some(Cost {
        nano_usd: parse(&text).ok_or(AgentError::Protocol(
            "invalid or overflowing reported service cost",
        ))?,
        source: CostSource::Reported,
    }))
}
pub(crate) fn wire_error(value: &Value) -> Option<AgentError> {
    let error = value.get("error")?.as_object()?;
    let status = error
        .get("code")
        .or_else(|| error.get("status"))
        .and_then(Value::as_u64)
        .and_then(|v| u16::try_from(v).ok())
        .filter(|n| (400..=599).contains(n));
    Some(AgentError::Remote(status.map_or(
        ProviderFailure {
            kind: FailureKind::Unavailable,
            http_status: None,
            retry_after_seconds: None,
        },
        |s| ProviderFailure::http(s, None),
    )))
}

#[cfg(test)]
mod tests;

/// Extract only the reported USD field without passing its numeric lexeme through f64.
pub(crate) fn raw_cost(bytes: &[u8]) -> Result<Option<Cost>, AgentError> {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        usage: Option<Usage>,
    }
    #[derive(Deserialize)]
    struct Usage {
        #[serde(default)]
        cost: Option<Box<serde_json::value::RawValue>>,
    }
    let envelope: Envelope = serde_json::from_slice(bytes)
        .map_err(|_| AgentError::Protocol("invalid service usage/cost envelope"))?;
    let Some(cost) = envelope.usage.and_then(|u| u.cost) else {
        return Ok(None);
    };
    let raw = cost.get();
    let value = if raw.starts_with('"') {
        serde_json::from_str::<String>(raw)
            .map_err(|_| AgentError::Protocol("invalid service cost string"))?
    } else {
        raw.into()
    };
    reported_cost(&Value::String(value))
}
