//! Owned, offline Docker computers. No host tool fallback, mounts or model credentials.
pub mod engine;
pub mod protocol;
mod tools;
use base64::{Engine as _, engine::general_purpose::STANDARD};
pub use engine::{Answer, ContainerInfo, Docker, Engine, Mount, Operation, VolumeInfo};
use namzu_core::{AgentError, Cancellation, TurnContext};
use namzu_session::{PrivateStore, StoreLock, now_ms};
use namzu_tools::process::ProcessOutput;
use protocol::{FILE_LIMIT, GuestRequest, GuestValue, valid_path};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Mutex, time::Duration};
pub use tools::{GuestPolicy, GuestTools};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ComputerSpec {
    pub image: String,
    pub memory_bytes: u64,
    pub cpu_millis: u32,
    pub pids: u32,
}
impl ComputerSpec {
    pub fn new(image: impl Into<String>) -> Self {
        Self {
            image: image.into(),
            memory_bytes: 256 * 1024 * 1024,
            cpu_millis: 1000,
            pids: 64,
        }
    }
    pub fn validate(&self) -> Result<(), AgentError> {
        if !self
            .image
            .strip_prefix("sha256:")
            .is_some_and(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
            || !(32 * 1024 * 1024..=2 * 1024 * 1024 * 1024).contains(&self.memory_bytes)
            || !(100..=4000).contains(&self.cpu_millis)
            || !(16..=256).contains(&self.pids)
        {
            return Err(AgentError::Configuration(
                "pinned local image ID and bounded memory/CPU/PID limits required",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Creating,
    Starting,
    Running,
    Stopping,
    Stopped,
    CleanupRequired,
    Closed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pending {
    pub kind: String,
    pub started_ms: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComputerRecord {
    pub version: u32,
    pub owner: String,
    pub pal_id: String,
    pub scope: String,
    pub generation: u64,
    pub container: String,
    pub volume: String,
    pub spec: ComputerSpec,
    pub phase: Phase,
    pub pending: Option<Pending>,
}
fn identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
}
pub fn labels(record: &ComputerRecord) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("io.namzu.computer.scope".into(), record.scope.clone()),
        ("io.namzu.computer.owner".into(), record.owner.clone()),
        ("io.namzu.computer.pal".into(), record.pal_id.clone()),
        (
            "io.namzu.computer.generation".into(),
            record.generation.to_string(),
        ),
    ])
}
fn random_scope() -> Result<String, AgentError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|_| AgentError::State("cannot create computer identity"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn cleanup_context() -> TurnContext {
    TurnContext::new(Duration::from_secs(20), Cancellation::default())
        .expect("fixed cleanup timeout")
}
pub struct ComputerManager<E = Docker> {
    store: PrivateStore,
    owner: String,
    pal_id: String,
    engine: Mutex<E>,
}
impl<E: Engine> ComputerManager<E> {
    pub fn from_store(
        store: PrivateStore,
        owner: &str,
        pal_id: &str,
        engine: E,
    ) -> Result<Self, AgentError> {
        if !identity(owner) || !identity(pal_id) {
            return Err(AgentError::Configuration("invalid computer owner/Pal ID"));
        }
        Ok(Self {
            store,
            owner: owner.into(),
            pal_id: pal_id.into(),
            engine: Mutex::new(engine),
        })
    }
    fn perform(&self, op: Operation, ctx: &TurnContext) -> Result<Answer, AgentError> {
        self.engine
            .lock()
            .map_err(|_| AgentError::State("computer engine lock poisoned"))?
            .perform(op, ctx)
    }
    fn load(&self) -> Result<Option<ComputerRecord>, AgentError> {
        let record: Option<ComputerRecord> = self.store.read("computer.json")?;
        if let Some(r) = &record {
            r.spec.validate()?;
            if r.version != 1
                || r.owner != self.owner
                || r.pal_id != self.pal_id
                || r.scope.len() != 32
                || !r.scope.bytes().all(|b| b.is_ascii_hexdigit())
                || r.generation == 0
                || r.container != format!("nr-{}-{}", r.scope, r.generation)
                || r.volume != format!("{}-work", r.container)
            {
                return Err(AgentError::State("computer identity mismatch"));
            }
        }
        Ok(record)
    }
    fn save(&self, record: &ComputerRecord) -> Result<(), AgentError> {
        self.store.write("computer.json", record)
    }
    pub fn snapshot(&self) -> Result<Option<ComputerRecord>, AgentError> {
        self.load()
    }
    pub fn acquire(
        &self,
        spec: ComputerSpec,
        ctx: &TurnContext,
    ) -> Result<Computer<'_, E>, AgentError> {
        spec.validate()?;
        ctx.remaining()?;
        let lock = self.store.lock("computer")?;
        let old = self.load()?;
        if old.as_ref().is_some_and(|r| r.phase != Phase::Closed) {
            return Err(AgentError::State(
                "computer exists; explicitly resume or recover cleanup",
            ));
        }
        if !matches!(
            self.perform(Operation::Image(spec.image.clone()), ctx)?,
            Answer::Image(true)
        ) {
            return Err(AgentError::Configuration(
                "local image must contain guest protocol 1",
            ));
        }
        let scope = old
            .as_ref()
            .map(|r| Ok(r.scope.clone()))
            .unwrap_or_else(random_scope)?;
        let generation = old
            .as_ref()
            .map_or(Some(1), |r| r.generation.checked_add(1))
            .ok_or(AgentError::State("computer generation exhausted"))?;
        let container = format!("nr-{scope}-{generation}");
        let record = ComputerRecord {
            version: 1,
            owner: self.owner.clone(),
            pal_id: self.pal_id.clone(),
            scope,
            generation,
            volume: format!("{container}-work"),
            container,
            spec,
            phase: Phase::Creating,
            pending: None,
        };
        self.save(&record)?;
        let mut computer = Computer {
            manager: self,
            record,
            _lock: lock,
            retained: false,
            preserve_workspace: false,
        };
        let result = (|| {
            self.done(
                Operation::CreateVolume {
                    record: computer.record.clone(),
                },
                ctx,
            )?;
            self.owned_volume(&computer.record, ctx)?
                .ok_or(AgentError::State("created volume missing"))?;
            self.done(
                Operation::Create {
                    record: computer.record.clone(),
                },
                ctx,
            )?;
            let info = self
                .owned_container(&computer.record, ctx)?
                .ok_or(AgentError::State("created container missing"))?;
            if !info.confined(&computer.record.spec, &computer.record.volume) {
                return Err(AgentError::State("container confinement mismatch"));
            }
            computer.record.phase = Phase::Stopped;
            self.save(&computer.record)?;
            computer.start(ctx)
        })();
        if let Err(error) = result {
            if self
                .cleanup(&mut computer.record, &cleanup_context())
                .is_ok()
            {
                computer.retained = true;
            } else {
                computer.record.phase = Phase::CleanupRequired;
                let _ = self.save(&computer.record);
                computer.retained = true;
                return Err(AgentError::State(
                    "computer creation failed; cleanup required",
                ));
            }
            return Err(error);
        }
        Ok(computer)
    }
    pub fn resume(&self, ctx: &TurnContext) -> Result<Computer<'_, E>, AgentError> {
        ctx.remaining()?;
        let lock = self.store.lock("computer")?;
        let mut record = self
            .load()?
            .ok_or(AgentError::State("computer not found"))?;
        if !matches!(record.phase, Phase::Running | Phase::Stopped) {
            return Err(AgentError::State(
                "incomplete lifecycle: explicit cleanup required",
            ));
        }
        let info = self
            .owned_container(&record, ctx)?
            .ok_or(AgentError::State(
                "owned container missing; cleanup required",
            ))?;
        self.owned_volume(&record, ctx)?
            .ok_or(AgentError::State("owned volume missing; cleanup required"))?;
        if !info.confined(&record.spec, &record.volume) {
            return Err(AgentError::State("container confinement changed"));
        }
        // Previous process may have died during an effect: stop it before handing over.
        if record.pending.is_some() || !info.running {
            self.stop_record(&mut record, ctx)?;
        }
        Ok(Computer {
            manager: self,
            record,
            _lock: lock,
            retained: false,
            preserve_workspace: false,
        })
    }
    pub fn recover_cleanup(&self, ctx: &TurnContext) -> Result<(), AgentError> {
        let _lock = self.store.lock("computer")?;
        if let Some(mut record) = self.load()? {
            self.cleanup(&mut record, ctx)?;
        }
        Ok(())
    }
    fn done(&self, op: Operation, ctx: &TurnContext) -> Result<(), AgentError> {
        if !matches!(self.perform(op, ctx)?, Answer::Done) {
            return Err(AgentError::Protocol("unexpected computer engine response"));
        }
        Ok(())
    }
    fn owned_container(
        &self,
        record: &ComputerRecord,
        ctx: &TurnContext,
    ) -> Result<Option<ContainerInfo>, AgentError> {
        let Answer::Container(info) =
            self.perform(Operation::Inspect(record.container.clone()), ctx)?
        else {
            return Err(AgentError::Protocol("invalid container inspection"));
        };
        if let Some(info) = &info
            && (info.name.trim_start_matches('/') != record.container
                || !labels(record)
                    .iter()
                    .all(|(k, v)| info.labels.get(k) == Some(v))
                || info.id.len() != 64
                || !info.id.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(AgentError::State(
                "container ownership mismatch; resource untouched",
            ));
        }
        Ok(info.map(|i| *i))
    }
    fn owned_volume(
        &self,
        record: &ComputerRecord,
        ctx: &TurnContext,
    ) -> Result<Option<VolumeInfo>, AgentError> {
        let Answer::Volume(info) =
            self.perform(Operation::VolumeInspect(record.volume.clone()), ctx)?
        else {
            return Err(AgentError::Protocol("invalid volume inspection"));
        };
        if let Some(info) = &info
            && (info.name != record.volume
                || !labels(record)
                    .iter()
                    .all(|(k, v)| info.labels.get(k) == Some(v)))
        {
            return Err(AgentError::State(
                "volume ownership mismatch; resource untouched",
            ));
        }
        Ok(info)
    }
    fn stop_record(
        &self,
        record: &mut ComputerRecord,
        ctx: &TurnContext,
    ) -> Result<(), AgentError> {
        if record.phase == Phase::Closed {
            return Ok(());
        }
        record.phase = Phase::Stopping;
        self.save(record)?;
        if let Some(info) = self.owned_container(record, ctx)? {
            if info.running {
                self.done(Operation::Kill(info.id), ctx)?;
            }
            if self
                .owned_container(record, ctx)?
                .is_some_and(|i| i.running)
            {
                return Err(AgentError::State("computer still running after stop"));
            }
        }
        record.phase = Phase::Stopped;
        self.save(record)
    }
    fn cleanup(&self, record: &mut ComputerRecord, ctx: &TurnContext) -> Result<(), AgentError> {
        if record.phase == Phase::Closed {
            return Ok(());
        }
        // Inspect BOTH labels before deleting either resource.
        let container = self.owned_container(record, ctx)?;
        self.owned_volume(record, ctx)?;
        record.phase = Phase::CleanupRequired;
        self.save(record)?;
        if let Some(info) = container {
            self.done(Operation::Remove(info.id), ctx)?;
        }
        if self.owned_container(record, ctx)?.is_some() {
            return Err(AgentError::State("container cleanup incomplete"));
        }
        if self.owned_volume(record, ctx)?.is_some() {
            self.done(Operation::RemoveVolume(record.volume.clone()), ctx)?;
        }
        if self.owned_volume(record, ctx)?.is_some() {
            return Err(AgentError::State("volume cleanup incomplete"));
        }
        record.phase = Phase::Closed;
        record.pending = None;
        self.save(record)
    }
}
pub struct Computer<'a, E: Engine = Docker> {
    manager: &'a ComputerManager<E>,
    record: ComputerRecord,
    _lock: StoreLock,
    retained: bool,
    preserve_workspace: bool,
}
impl<E: Engine> Computer<'_, E> {
    /// A persistent host session keeps files on early return/unwind, while Drop
    /// still stops remote work with an independent bounded cleanup context.
    pub fn preserve_workspace_on_drop(&mut self) {
        self.preserve_workspace = true;
    }
    pub fn snapshot(&self) -> &ComputerRecord {
        &self.record
    }
    pub fn start(&mut self, ctx: &TurnContext) -> Result<(), AgentError> {
        ctx.remaining()?;
        if self.record.phase != Phase::Stopped {
            return Err(AgentError::State("start requires stopped computer"));
        }
        let info = self
            .manager
            .owned_container(&self.record, ctx)?
            .ok_or(AgentError::State("owned computer missing"))?;
        if !info.confined(&self.record.spec, &self.record.volume) {
            return Err(AgentError::State("container confinement changed"));
        }
        self.record.phase = Phase::Starting;
        self.manager.save(&self.record)?;
        let result = (|| {
            self.manager.done(Operation::Start(info.id), ctx)?;
            if !self
                .manager
                .owned_container(&self.record, ctx)?
                .is_some_and(|i| i.running)
            {
                return Err(AgentError::State("computer did not start"));
            }
            // No user command is replayed by a start/restart.
            self.record.phase = Phase::Running;
            self.manager.save(&self.record)?;
            if !matches!(self.execute(GuestRequest::Probe, ctx)?, GuestValue::Ready) {
                return Err(AgentError::Protocol("guest handshake failed"));
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = self
                .manager
                .stop_record(&mut self.record, &cleanup_context());
        }
        result
    }
    pub fn stop(&mut self, ctx: &TurnContext) -> Result<(), AgentError> {
        self.manager.stop_record(&mut self.record, ctx)
    }
    pub fn restart(&mut self, ctx: &TurnContext) -> Result<(), AgentError> {
        self.stop(ctx)?;
        self.start(ctx)
    }
    pub fn close(&mut self, ctx: &TurnContext) -> Result<(), AgentError> {
        let result = self.manager.cleanup(&mut self.record, ctx);
        if result.is_ok() {
            self.retained = true;
        }
        result
    }
    /// Explicit host control: release the client lease while retaining an idle,
    /// healthy computer and its volume. Running PID1 remains resource constrained.
    /// User work must already have completed; pending effects cannot be released.
    pub fn release(mut self, ctx: &TurnContext) -> Result<ComputerRecord, AgentError> {
        ctx.remaining()?;
        if self.record.pending.is_some()
            || !matches!(self.record.phase, Phase::Running | Phase::Stopped)
        {
            return Err(AgentError::State(
                "only a healthy idle computer can be released",
            ));
        }
        self.retained = true;
        Ok(self.record.clone())
    }
    pub fn detach(mut self, ctx: &TurnContext) -> Result<ComputerRecord, AgentError> {
        self.stop(ctx)?;
        self.retained = true;
        Ok(self.record.clone())
    }
    pub fn acknowledge_uncertain(&mut self) -> Result<(), AgentError> {
        if !matches!(self.record.phase, Phase::Stopped | Phase::Running) {
            return Err(AgentError::State(
                "computer cannot acknowledge this lifecycle state",
            ));
        }
        self.record.pending = None;
        self.manager.save(&self.record)
    }
    fn abort_remote(&mut self) -> Result<(), AgentError> {
        if self
            .manager
            .stop_record(&mut self.record, &cleanup_context())
            .is_err()
        {
            self.record.phase = Phase::CleanupRequired;
            let _ = self.manager.save(&self.record);
            return Err(AgentError::State(
                "remote stop failed; computer cleanup required",
            ));
        }
        Ok(())
    }
    fn execute(
        &mut self,
        request: GuestRequest,
        ctx: &TurnContext,
    ) -> Result<GuestValue, AgentError> {
        ctx.remaining()?;
        if self.record.phase != Phase::Running {
            return Err(AgentError::State(
                "computer is not running; no host fallback",
            ));
        }
        let info = self
            .manager
            .owned_container(&self.record, ctx)?
            .ok_or(AgentError::State("owned computer missing"))?;
        if !info.running || !info.confined(&self.record.spec, &self.record.volume) {
            return Err(AgentError::State("computer confinement/liveness changed"));
        }
        let readonly = matches!(
            request,
            GuestRequest::Probe | GuestRequest::Read { .. } | GuestRequest::List { .. }
        );
        if self.record.pending.is_some() && !readonly {
            return Err(AgentError::State(
                "uncertain effect: inspect and acknowledge before new writes/shell",
            ));
        }
        let previous = self.record.pending.clone();
        let kind = match &request {
            GuestRequest::Probe => "probe",
            GuestRequest::Read { .. } => "read",
            GuestRequest::Write { .. } => "write",
            GuestRequest::List { .. } => "list",
            GuestRequest::Shell { .. } => "shell",
        };
        if previous.is_none() {
            self.record.pending = Some(Pending {
                kind: kind.into(),
                started_ms: now_ms(),
            });
            self.manager.save(&self.record)?;
        }
        let reply =
            self.manager
                .perform(
                    Operation::Exec {
                        container: info.id,
                        request,
                    },
                    ctx,
                )
                .and_then(|answer| {
                    ctx.remaining()?;
                    match answer {
                        Answer::Guest(reply) if reply.version == 1 => {
                            if let Ok(value) = &reply.result {
                                let valid = match (kind, value) {
                                    ("probe", GuestValue::Ready)
                                    | ("write", GuestValue::Written) => true,
                                    ("read", GuestValue::Bytes { base64 }) => {
                                        base64.len() <= FILE_LIMIT.div_ceil(3) * 4
                                            && STANDARD
                                                .decode(base64)
                                                .is_ok_and(|b| b.len() <= FILE_LIMIT)
                                    }
                                    ("list", GuestValue::Entries { names }) => {
                                        names.len() <= 512
                                            && names.iter().all(|s| {
                                                !s.is_empty()
                                                    && s.len() <= 255
                                                    && !s.contains(['/', '\0'])
                                            })
                                    }
                                    ("shell", GuestValue::Shell { output }) => {
                                        ["exited", "timeout", "output_limit"]
                                            .contains(&output.status.as_str())
                                            && output.stdout.len() + output.stderr.len()
                                                <= 3 * FILE_LIMIT
                                    }
                                    _ => false,
                                };
                                if !valid {
                                    return Err(AgentError::Protocol(
                                        "unexpected or oversized guest response",
                                    ));
                                }
                            }
                            Ok(reply.result)
                        }
                        _ => Err(AgentError::Protocol("guest protocol mismatch")),
                    }
                });
        match reply {
            Ok(result) => {
                if (kind == "shell" || kind == "write") && result.is_err() {
                    self.abort_remote()?;
                    return Err(AgentError::State(
                        "guest effect failed; computer stopped; inspect effects",
                    ));
                }
                let pending = self.record.pending.clone();
                self.record.pending = previous;
                if let Err(error) = self.manager.save(&self.record) {
                    self.record.pending = pending;
                    self.abort_remote()?;
                    return Err(error);
                }
                result.map_err(|_| AgentError::State("guest rejected operation; inspect workspace"))
            }
            Err(error) => {
                self.abort_remote()?;
                Err(error)
            }
        }
    }
    pub fn upload(&mut self, path: &str, data: &[u8], ctx: &TurnContext) -> Result<(), AgentError> {
        if !valid_path(path) || data.len() > FILE_LIMIT {
            return Err(AgentError::Configuration(
                "bounded relative guest file required",
            ));
        }
        match self.execute(
            GuestRequest::Write {
                path: path.into(),
                base64: STANDARD.encode(data),
            },
            ctx,
        )? {
            GuestValue::Written => Ok(()),
            _ => Err(AgentError::Protocol("unexpected guest write response")),
        }
    }
    pub fn download(&mut self, path: &str, ctx: &TurnContext) -> Result<Vec<u8>, AgentError> {
        if !valid_path(path) {
            return Err(AgentError::Configuration("relative guest file required"));
        }
        let GuestValue::Bytes { base64 } =
            self.execute(GuestRequest::Read { path: path.into() }, ctx)?
        else {
            return Err(AgentError::Protocol("unexpected guest read response"));
        };
        if base64.len() > FILE_LIMIT.div_ceil(3) * 4 {
            return Err(AgentError::Protocol("guest file exceeds limit"));
        }
        let bytes = STANDARD
            .decode(base64)
            .map_err(|_| AgentError::Protocol("invalid guest file encoding"))?;
        if bytes.len() > FILE_LIMIT {
            return Err(AgentError::Protocol("guest file exceeds limit"));
        }
        Ok(bytes)
    }
    pub fn list(&mut self, path: &str, ctx: &TurnContext) -> Result<Vec<String>, AgentError> {
        if !valid_path(path) {
            return Err(AgentError::Configuration(
                "relative guest directory required",
            ));
        }
        let GuestValue::Entries { names } =
            self.execute(GuestRequest::List { path: path.into() }, ctx)?
        else {
            return Err(AgentError::Protocol("unexpected guest list response"));
        };
        if names.len() > 512 {
            return Err(AgentError::Protocol("guest directory limit"));
        }
        Ok(names)
    }
    pub fn shell(
        &mut self,
        command: &str,
        cwd: &str,
        timeout: Duration,
        ctx: &TurnContext,
    ) -> Result<ProcessOutput, AgentError> {
        if command.is_empty()
            || command.len() > FILE_LIMIT
            || !valid_path(cwd)
            || timeout.is_zero()
            || timeout > Duration::from_secs(300)
        {
            return Err(AgentError::Configuration("invalid guest shell request"));
        }
        let timeout_ms = timeout.min(ctx.remaining()?).as_millis().min(300_000) as u64;
        if timeout_ms == 0 {
            return Err(AgentError::Timeout);
        }
        let GuestValue::Shell { output } = self.execute(
            GuestRequest::Shell {
                command: command.into(),
                cwd: cwd.into(),
                timeout_ms,
            },
            ctx,
        )?
        else {
            return Err(AgentError::Protocol("unexpected guest shell response"));
        };
        if output.status != "exited" {
            self.manager
                .stop_record(&mut self.record, &cleanup_context())?;
        }
        Ok(output)
    }
}
impl<E: Engine> Drop for Computer<'_, E> {
    fn drop(&mut self) {
        if !self.retained && self.record.phase != Phase::Closed {
            let ctx = cleanup_context();
            let incomplete = matches!(
                self.record.phase,
                Phase::CleanupRequired | Phase::Creating | Phase::Starting | Phase::Stopping
            );
            if self.preserve_workspace
                || self.record.pending.is_some()
                || matches!(
                    self.record.phase,
                    Phase::CleanupRequired | Phase::Creating | Phase::Starting | Phase::Stopping
                )
            {
                if self.manager.stop_record(&mut self.record, &ctx).is_err() || incomplete {
                    self.record.phase = Phase::CleanupRequired;
                    let _ = self.manager.save(&self.record);
                }
            } else if self.manager.cleanup(&mut self.record, &ctx).is_err() {
                self.record.phase = Phase::CleanupRequired;
                let _ = self.manager.save(&self.record);
            }
        }
    }
}
