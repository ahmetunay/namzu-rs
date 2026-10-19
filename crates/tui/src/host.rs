use crate::Outcome;
use namzu_core::{Cancellation, EventSink};

#[derive(Clone)]
pub struct Controls {
    pub provider: String,
    pub model: String,
    pub effort: String,
    pub timeout: u64,
    pub steps: usize,
    pub tool_calls: usize,
    pub context_bytes: usize,
}
impl Default for Controls {
    fn default() -> Self {
        Self {
            provider: String::new(),
            model: String::new(),
            effort: String::new(),
            timeout: 300,
            steps: 64,
            tool_calls: 128,
            context_bytes: 4 * 1024 * 1024,
        }
    }
}
#[derive(Clone)]
pub struct SessionSummary {
    pub id: String,
    pub model: String,
    pub status: String,
    pub updated_ms: u64,
}
pub struct HistoryEntry {
    pub image: Option<namzu_core::ToolImage>,
    pub label: &'static str,
    pub content: String,
    pub tool_id: Option<String>,
}
pub struct History {
    pub id: String,
    pub entries: Vec<HistoryEntry>,
    pub resumable: bool,
    pub notice: String,
    pub context_bytes: usize,
}

/// Local management is separate from network inference. No credentials or opaque
/// replay state belong in these presentation types. Effects still use SDK policy.
pub trait Host: Send + Sync {
    fn turn(
        &self,
        prompt: String,
        session: Option<String>,
        cancellation: Cancellation,
        events: &mut dyn EventSink,
    ) -> Outcome;
    fn controls(&self) -> Result<Controls, String> {
        Ok(Controls::default())
    }
    fn apply(&self, _: Controls) -> Result<(), String> {
        Err("settings are not available".into())
    }
    fn sessions(&self) -> Result<Vec<SessionSummary>, String> {
        Ok(vec![])
    }
    fn history(&self, _: &str) -> Result<History, String> {
        Err("session history is not available".into())
    }
}

pub(crate) struct FunctionHost<F>(pub F);
impl<F> Host for FunctionHost<F>
where
    F: Fn(String, Option<String>, Cancellation, &mut dyn EventSink) -> Outcome + Send + Sync,
{
    fn turn(
        &self,
        prompt: String,
        session: Option<String>,
        cancellation: Cancellation,
        events: &mut dyn EventSink,
    ) -> Outcome {
        self.0(prompt, session, cancellation, events)
    }
}

pub(crate) struct Panel {
    pub fields: Vec<String>,
    pub selected: usize,
    pub error: String,
    provider: String,
}
impl Panel {
    pub fn new(c: Controls) -> Self {
        Self {
            fields: vec![
                c.model,
                c.effort,
                c.timeout.to_string(),
                c.steps.to_string(),
                c.tool_calls.to_string(),
                c.context_bytes.to_string(),
            ],
            selected: 0,
            error: String::new(),
            provider: c.provider,
        }
    }
    pub fn parse(&self) -> Result<Controls, String> {
        let number = |i: usize| {
            self.fields[i]
                .parse::<usize>()
                .map_err(|_| "Enter positive integer budgets".to_string())
        };
        let c = Controls {
            provider: self.provider.clone(),
            model: self.fields[0].clone(),
            effort: self.fields[1].clone(),
            timeout: number(2)? as u64,
            steps: number(3)?,
            tool_calls: number(4)?,
            context_bytes: number(5)?,
        };
        if c.model.is_empty()
            || c.model.len() > 256
            || !(1..=86400).contains(&c.timeout)
            || !(1..=10000).contains(&c.steps)
            || c.tool_calls > 10000
            || !(1..=4 * 1024 * 1024).contains(&c.context_bytes)
        {
            return Err("Model required; timeout 1–86400, steps 1–10000, tools 0–10000, history 1–4194304 bytes".into());
        }
        if !c.effort.is_empty()
            && (c.provider != "codex"
                || ![
                    "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
                ]
                .contains(&c.effort.as_str()))
        {
            return Err(
                "Effort must be empty for API providers, or a supported Codex effort".into(),
            );
        }
        Ok(c)
    }
}
