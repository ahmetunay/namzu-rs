//! Versioned, content-free local traces. No exporter, arbitrary attributes or network.
//! Raw presentation events are projected directly into typed numeric/enum fields.
use namzu_core::{
    AgentError, Event, EventSink, Message, Observer, Stats, ToolCall, Turn, TurnContext,
    UsageTotals,
    provider::{FailureKind, ResponseMetadata},
};
use namzu_session::PrivateStore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Instant;

pub const MAX_RECORDS: usize = 1024;
pub const MAX_TRACE_BYTES: usize = 1024 * 1024;
fn invalid() -> AgentError {
    AgentError::State("invalid or excessive telemetry trace")
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(try_from = "String", into = "String")]
pub struct TraceId(String);
impl TraceId {
    pub fn generate() -> Self {
        Self(format!(
            "t-{:x}",
            Sha256::digest(namzu_session::new_id().as_bytes())
        ))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for TraceId {
    type Error = AgentError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        if s.len() == 66
            && s.starts_with("t-")
            && s[2..]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            Ok(Self(s))
        } else {
            Err(invalid())
        }
    }
}
impl From<TraceId> for String {
    fn from(id: TraceId) -> String {
        id.0
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Exec,
    Agui,
    Tui,
    Pal,
    Sdk,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolClass {
    Files,
    Shell,
    Mcp,
    Audio,
    Lsp,
    Computer,
    Browser,
    Other,
}
impl ToolClass {
    fn of(name: &str) -> Self {
        match name {
            "read_file" | "write_file" | "list_files" | "search_files" | "diff_file"
            | "apply_patch" | "inspect_file" | "read_file_chunk" | "read_document"
            | "read_image" => Self::Files,
            "shell" => Self::Shell,
            s if s.starts_with("mcp_") => Self::Mcp,
            s if s.starts_with("audio_") => Self::Audio,
            s if s.starts_with("lsp_") => Self::Lsp,
            s if s.starts_with("computer_") => Self::Computer,
            s if s.starts_with("browser_") => Self::Browser,
            _ => Self::Other,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Failure {
    Cancelled,
    Timeout,
    StepBudget,
    ToolBudget,
    Configuration,
    Protocol,
    Provider,
    State,
    Remote {
        class: FailureKind,
        http_status: Option<u16>,
    },
}
impl Failure {
    pub fn of(error: &AgentError) -> Self {
        match error {
            AgentError::Cancelled => Self::Cancelled,
            AgentError::Timeout => Self::Timeout,
            AgentError::StepLimit => Self::StepBudget,
            AgentError::ToolLimit => Self::ToolBudget,
            AgentError::Configuration(_) => Self::Configuration,
            AgentError::Protocol(_) => Self::Protocol,
            AgentError::Provider(_) => Self::Provider,
            AgentError::State(_) => Self::State,
            AgentError::Remote(e) => Self::Remote {
                class: e.kind,
                http_status: e.http_status,
            },
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum Outcome {
    Running,
    Ok,
    ToolError,
    Interrupted,
    Failed { failure: Failure },
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum SpanKind {
    Turn,
    Provider,
    Tool { class: ToolClass },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TraceEvent {
    Started { kind: SpanKind },
    Ended { outcome: Outcome, duration_ms: u64 },
    Usage { metadata: ResponseMetadata },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub sequence: u64,
    pub at_ms: u64,
    pub span: u64,
    pub parent: Option<u64>,
    pub event: TraceEvent,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Timing {
    pub samples: u64,
    pub sum_ms: u64,
    pub max_ms: u64,
}
impl Timing {
    fn add(&mut self, ms: u64) {
        self.samples = self.samples.saturating_add(1);
        self.sum_ms = self.sum_ms.saturating_add(ms);
        self.max_ms = self.max_ms.max(ms);
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Metrics {
    pub provider_started: u64,
    pub provider_completed: u64,
    pub tools_started: u64,
    pub tools_completed: u64,
    pub tool_errors: u64,
    pub approvals_allowed: u64,
    pub approvals_denied: u64,
    pub delta_events: u64,
    pub delta_utf8_bytes: u64,
    pub dropped_records: u64,
    pub invalid_events: u64,
    pub provider_duration: Timing,
    pub tool_duration: Timing,
    pub usage: UsageTotals,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Trace {
    pub schema_version: u32,
    pub trace_id: TraceId,
    pub source: Source,
    pub started_unix_ms: u64,
    pub elapsed_ms: u64,
    pub outcome: Outcome,
    pub records: Vec<Record>,
    pub metrics: Metrics,
}
impl Trace {
    pub fn validate(&self) -> Result<(), AgentError> {
        if self.schema_version != 1 || self.records.is_empty() || self.records.len() > MAX_RECORDS {
            return Err(invalid());
        }
        let mut last = 0;
        for (i, r) in self.records.iter().enumerate() {
            if r.sequence != i as u64
                || r.at_ms < last
                || r.at_ms > self.elapsed_ms
                || r.span == 0
                || r.parent != if r.span == 1 { None } else { Some(1) }
            {
                return Err(invalid());
            }
            if let TraceEvent::Usage { metadata } = &r.event {
                metadata.validate()?;
            }
            if matches!(
                &r.event,
                TraceEvent::Ended {
                    outcome: Outcome::Running,
                    ..
                }
            ) || matches!(&r.event, TraceEvent::Ended {duration_ms,..} if *duration_ms > r.at_ms)
            {
                return Err(invalid());
            }
            last = r.at_ms;
        }
        if !matches!(
            self.records[0].event,
            TraceEvent::Started {
                kind: SpanKind::Turn
            }
        ) || self.records[0].span != 1
        {
            return Err(invalid());
        }
        if self.outcome != Outcome::Running
            && !matches!(self.records.last(),Some(Record{span:1,event:TraceEvent::Ended {outcome,..},..}) if outcome == &self.outcome)
        {
            return Err(invalid());
        }
        if serde_json::to_vec(self).map_err(|_| invalid())?.len() > MAX_TRACE_BYTES {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Copy)]
struct Active {
    id: u64,
    started: u64,
}
/// One turn, bounded memory, no retained raw event/text/ID/argument buffers.
pub struct Recorder {
    trace: Trace,
    started: Instant,
    cap: usize,
    next: u64,
    provider: Option<Active>,
    tool: Option<Active>,
    finished: bool,
}
impl Recorder {
    pub fn new(source: Source, cap: usize) -> Result<Self, AgentError> {
        if !(2..=MAX_RECORDS).contains(&cap) {
            return Err(invalid());
        }
        Ok(Self {
            trace: Trace {
                schema_version: 1,
                trace_id: TraceId::generate(),
                source,
                started_unix_ms: namzu_session::now_ms(),
                elapsed_ms: 0,
                outcome: Outcome::Running,
                records: vec![Record {
                    sequence: 0,
                    at_ms: 0,
                    span: 1,
                    parent: None,
                    event: TraceEvent::Started {
                        kind: SpanKind::Turn,
                    },
                }],
                metrics: Metrics::default(),
            },
            started: Instant::now(),
            cap,
            next: 2,
            provider: None,
            tool: None,
            finished: false,
        })
    }
    fn now(&self) -> u64 {
        self.started.elapsed().as_millis().min(u64::MAX as u128) as u64
    }
    pub fn snapshot(&self) -> &Trace {
        &self.trace
    }
    fn time(&mut self, ms: u64) -> u64 {
        if ms < self.trace.elapsed_ms {
            self.trace.metrics.invalid_events = self.trace.metrics.invalid_events.saturating_add(1);
        }
        self.trace.elapsed_ms = self.trace.elapsed_ms.max(ms);
        self.trace.elapsed_ms
    }
    fn push(&mut self, span: u64, event: TraceEvent) {
        if self.trace.records.len() >= self.cap - 1 {
            self.trace.metrics.dropped_records =
                self.trace.metrics.dropped_records.saturating_add(1);
            return;
        }
        self.trace.records.push(Record {
            sequence: self.trace.records.len() as u64,
            at_ms: self.trace.elapsed_ms,
            span,
            parent: Some(1),
            event,
        });
    }
    fn begin(&mut self, kind: SpanKind) -> Active {
        let a = Active {
            id: self.next,
            started: self.trace.elapsed_ms,
        };
        self.next = self.next.saturating_add(1);
        self.push(a.id, TraceEvent::Started { kind });
        a
    }
    fn end(&mut self, a: Active, outcome: Outcome) -> u64 {
        let ms = self.trace.elapsed_ms.saturating_sub(a.started);
        self.push(
            a.id,
            TraceEvent::Ended {
                outcome,
                duration_ms: ms,
            },
        );
        ms
    }
    fn complete_provider(&mut self) {
        if let Some(a) = self.provider.take() {
            let ms = self.end(a, Outcome::Ok);
            self.trace.metrics.provider_completed =
                self.trace.metrics.provider_completed.saturating_add(1);
            self.trace.metrics.provider_duration.add(ms);
        }
    }
    pub fn observe(&mut self, event: &Event) {
        self.observe_at(event, self.now());
    }
    /// Host clock adapter for deterministic tests. Regressions are clamped and counted.
    pub fn observe_at(&mut self, event: &Event, ms: u64) {
        if self.finished {
            return;
        }
        self.time(ms);
        match event {
            Event::ProviderStarted { .. } => {
                if let Some(a) = self.provider.take() {
                    self.end(a, Outcome::Interrupted);
                    self.trace.metrics.invalid_events =
                        self.trace.metrics.invalid_events.saturating_add(1);
                }
                self.provider = Some(self.begin(SpanKind::Provider));
                self.trace.metrics.provider_started =
                    self.trace.metrics.provider_started.saturating_add(1);
            }
            Event::ProviderFinished { metadata } => {
                if let Some(a) = self.provider {
                    if self.trace.metrics.usage.record(metadata).is_ok() {
                        self.push(
                            a.id,
                            TraceEvent::Usage {
                                metadata: metadata.clone(),
                            },
                        );
                    } else {
                        self.trace.metrics.invalid_events =
                            self.trace.metrics.invalid_events.saturating_add(1);
                    }
                    self.complete_provider();
                } else {
                    self.trace.metrics.invalid_events =
                        self.trace.metrics.invalid_events.saturating_add(1);
                }
            }
            Event::AssistantMessage { .. } => self.complete_provider(),
            Event::AssistantDelta { text } => {
                self.trace.metrics.delta_events = self.trace.metrics.delta_events.saturating_add(1);
                self.trace.metrics.delta_utf8_bytes = self
                    .trace
                    .metrics
                    .delta_utf8_bytes
                    .saturating_add(text.len() as u64);
            }
            Event::ToolStarted { name, .. } => {
                if let Some(a) = self.tool.take() {
                    self.end(a, Outcome::Interrupted);
                    self.trace.metrics.invalid_events =
                        self.trace.metrics.invalid_events.saturating_add(1);
                }
                self.tool = Some(self.begin(SpanKind::Tool {
                    class: ToolClass::of(name),
                }));
                self.trace.metrics.tools_started =
                    self.trace.metrics.tools_started.saturating_add(1);
            }
            Event::ToolFinished { result, .. } => {
                if let Some(a) = self.tool.take() {
                    let ms = self.end(
                        a,
                        if result.is_error {
                            Outcome::ToolError
                        } else {
                            Outcome::Ok
                        },
                    );
                    self.trace.metrics.tools_completed =
                        self.trace.metrics.tools_completed.saturating_add(1);
                    self.trace.metrics.tool_errors = self
                        .trace
                        .metrics
                        .tool_errors
                        .saturating_add(u64::from(result.is_error));
                    self.trace.metrics.tool_duration.add(ms);
                } else {
                    self.trace.metrics.invalid_events =
                        self.trace.metrics.invalid_events.saturating_add(1);
                }
            }
            // Terminal event error strings/stats are never copied. The actual typed host
            // result is supplied to finish, including post-agent cleanup/journal failures.
            Event::TurnStarted | Event::TurnCompleted { .. } | Event::TurnFailed { .. } => {}
        }
    }
    pub fn approval(&mut self, allowed: bool) {
        if !self.finished {
            let m = &mut self.trace.metrics;
            if allowed {
                m.approvals_allowed = m.approvals_allowed.saturating_add(1);
            } else {
                m.approvals_denied = m.approvals_denied.saturating_add(1);
            }
        }
    }
    pub fn finish(&mut self, error: Option<&AgentError>) {
        self.finish_at(error, self.now());
    }
    pub fn finish_at(&mut self, error: Option<&AgentError>, ms: u64) {
        if self.finished {
            return;
        }
        self.time(ms);
        let outcome = error.map_or(Outcome::Ok, |e| Outcome::Failed {
            failure: Failure::of(e),
        });
        for active in [self.provider.take(), self.tool.take()]
            .into_iter()
            .flatten()
        {
            self.end(
                active,
                if error.is_some() {
                    outcome.clone()
                } else {
                    Outcome::Interrupted
                },
            );
        }
        self.trace.outcome = outcome.clone();
        self.trace.records.push(Record {
            sequence: self.trace.records.len() as u64,
            at_ms: self.trace.elapsed_ms,
            span: 1,
            parent: None,
            event: TraceEvent::Ended {
                outcome,
                duration_ms: self.trace.elapsed_ms,
            },
        });
        self.finished = true;
    }
}
/// Event sink decorator preserves presentation/approval errors and the exact context.
pub struct Tap<'a> {
    pub inner: &'a mut dyn EventSink,
    pub recorder: Option<&'a mut Recorder>,
}
impl EventSink for Tap<'_> {
    fn tool_proposed(&mut self, call: &ToolCall) -> Result<(), AgentError> {
        self.inner.tool_proposed(call)
    }
    fn supports_approval(&self) -> bool {
        self.inner.supports_approval()
    }
    fn approve(&mut self, c: &ToolCall, ctx: &TurnContext) -> Result<bool, AgentError> {
        let allowed = self.inner.approve(c, ctx)?;
        if let Some(r) = &mut self.recorder {
            r.approval(allowed);
        }
        Ok(allowed)
    }
    fn emit(&mut self, e: Event) -> Result<(), AgentError> {
        if let Some(r) = &mut self.recorder {
            r.observe(&e);
        }
        self.inner.emit(e)
    }
}
pub struct Observed<'a, O> {
    pub inner: &'a mut O,
    pub recorder: Option<&'a mut Recorder>,
}
impl<O: Observer> Observer for Observed<'_, O> {
    fn supports_approval(&self) -> bool {
        self.inner.supports_approval()
    }
    fn approve(&mut self, c: &ToolCall, ctx: &TurnContext) -> Result<bool, AgentError> {
        let v = self.inner.approve(c, ctx)?;
        if let Some(r) = &mut self.recorder {
            r.approval(v);
        }
        Ok(v)
    }
    fn checkpoint(
        &mut self,
        m: &[Message],
        s: &Stats,
        p: Option<&ToolCall>,
    ) -> Result<(), AgentError> {
        self.inner.checkpoint(m, s, p)
    }
    fn event(&mut self, e: Event) -> Result<(), AgentError> {
        if let Some(r) = &mut self.recorder {
            r.observe(&e);
        }
        self.inner.event(e)
    }
}
/// Private atomic pending marker before effects, final snapshot after the host result.
/// No Drop flush: process death deliberately leaves outcome=running, never fake success.
pub struct LocalTrace {
    store: PrivateStore,
    pub recorder: Recorder,
}
impl LocalTrace {
    pub fn start(store: &PrivateStore, source: Source) -> Result<Self, AgentError> {
        let child = store.child("telemetry")?;
        let recorder = Recorder::new(source, MAX_RECORDS)?;
        child.write_new(
            &format!("{}.json", recorder.snapshot().trace_id.as_str()),
            recorder.snapshot(),
        )?;
        Ok(Self {
            store: child,
            recorder,
        })
    }
    pub fn finish(&mut self, error: Option<&AgentError>) -> Result<(), AgentError> {
        self.recorder.finish(error);
        self.recorder.snapshot().validate()?;
        self.store.write(
            &format!("{}.json", self.recorder.snapshot().trace_id.as_str()),
            self.recorder.snapshot(),
        )
    }
}
pub fn read(store: &PrivateStore, id: &TraceId) -> Result<Trace, AgentError> {
    let trace: Trace = store
        .open_child("telemetry")?
        .read(&format!("{}.json", id.as_str()))?
        .ok_or(AgentError::State("telemetry trace not found"))?;
    trace.validate()?;
    if &trace.trace_id != id {
        return Err(invalid());
    }
    Ok(trace)
}
/// Preserve an existing turn failure; publication failure on an otherwise successful
/// turn is explicit, without pretending that already committed tool effects rolled back.
pub fn finish_result(
    trace: &mut Option<LocalTrace>,
    result: Result<Turn, AgentError>,
) -> Result<Turn, AgentError> {
    if let Some(trace) = trace {
        let saved = trace.finish(result.as_ref().err());
        if saved.is_err() && result.is_ok() {
            return Err(AgentError::State(
                "telemetry publication failed; tools may have run; inspect session and pending trace",
            ));
        }
    }
    result
}
#[cfg(test)]
mod tests;
