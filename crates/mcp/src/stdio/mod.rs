//! Bounded Unix stdio client. One mutable owner drives all I/O; no detached reader
//! threads or hidden restarts. Spawn is explicit host execution, not a sandbox.
mod framing;
mod process;
use crate::{
    Abandoned, ClientSession, Id, Incoming, Limits, Message, Phase, types::Implementation,
};
use framing::Frames;
use namzu_core::{AgentError, TurnContext};
use process::{Process, nonblocking};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, VecDeque},
    io::{self, Read, Write},
    path::PathBuf,
    process::{ChildStderr, ChildStdin, ChildStdout},
    time::Duration,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid MCP stdio launch configuration")]
    Configuration,
    #[error("cannot launch MCP stdio process")]
    Spawn,
    #[error("MCP stdio I/O failed")]
    Io,
    #[error("MCP stdio reached EOF or process exit")]
    Eof,
    #[error("MCP stdio ended with an incomplete frame")]
    Truncated,
    #[error("MCP stdio frame exceeds the limit")]
    FrameLimit,
    #[error("MCP stdio queue exceeds the limit")]
    Backpressure,
    #[error("MCP stdio output budget exceeded")]
    OutputLimit,
    #[error("MCP stdio client is closed")]
    Closed,
    #[error("MCP stdio call requires no other pending request")]
    Busy,
    #[error(transparent)]
    Protocol(#[from] crate::Error),
    #[error("MCP stdio context ended: {0}")]
    Context(AgentError),
}
impl From<AgentError> for Error {
    fn from(e: AgentError) -> Self {
        Self::Context(e)
    }
}
/// Values are explicit host selections. No shell, package runner or automatic
/// downloads are injected. Environment values/arguments are never logged here.
pub struct Spec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub stdout_limit: usize,
    pub stderr_limit: usize,
    pub stderr_capture: usize,
    pub queue_bytes: usize,
    pub shutdown_grace: Duration,
}
impl Spec {
    pub fn new(program: impl Into<PathBuf>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: vec![],
            cwd: cwd.into(),
            env: BTreeMap::new(),
            stdout_limit: 16 * 1024 * 1024,
            stderr_limit: 1024 * 1024,
            stderr_capture: 16384,
            queue_bytes: 2 * 1024 * 1024,
            shutdown_grace: Duration::from_millis(100),
        }
    }
    fn validate(&self, limits: &Limits) -> Result<(), Error> {
        if !self.program.is_absolute()
            || !self.cwd.is_absolute()
            || !self.program.is_file()
            || !self.cwd.is_dir()
            || self.args.len() > 128
            || self.args.iter().map(String::len).sum::<usize>() > 65536
            || self.args.iter().any(|s| s.contains('\0'))
            || self.env.len() > 64
            || self.env.iter().any(|(k, v)| {
                k.is_empty()
                    || k.len() > 128
                    || !k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    || v.len() > 16384
                    || v.contains('\0')
            })
            || !(1..=64 * 1024 * 1024).contains(&self.stdout_limit)
            || !(1..=16 * 1024 * 1024).contains(&self.stderr_limit)
            || self.stderr_capture > 65536
            || self.stderr_capture > self.stderr_limit
            || self.queue_bytes < limits.message_bytes
            || self.queue_bytes > 8 * 1024 * 1024
            || self.shutdown_grace > Duration::from_secs(2)
        {
            return Err(Error::Configuration);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloseReport {
    pub exit_code: Option<i32>,
    pub forced: bool,
    pub abandoned: Vec<Abandoned>,
    pub stderr_bytes: usize,
    pub stderr_truncated: bool,
}
struct Outgoing {
    bytes: Vec<u8>,
    offset: usize,
}
/// Thread-affine connection owner. Linux parent-death signals follow the spawning
/// thread, so the connection must be created and driven on one persistent thread.
///
/// ```compile_fail
/// fn requires_send<T: Send>() {}
/// requires_send::<namzu_mcp::stdio::Client>();
/// ```
pub struct Client {
    thread_bound: std::marker::PhantomData<std::rc::Rc<()>>,
    session: ClientSession,
    process: Process,
    input: Option<ChildStdin>,
    output: ChildStdout,
    error: ChildStderr,
    lifetime: TurnContext,
    frames: Frames,
    writes: VecDeque<Outgoing>,
    queued: usize,
    queue_limit: usize,
    stdout_bytes: usize,
    stdout_limit: usize,
    stderr_bytes: usize,
    stderr_limit: usize,
    stderr_capture: usize,
    stderr: VecDeque<u8>,
    eof: bool,
    grace: Duration,
    outstanding: BTreeMap<Id, String>,
    closed: Option<CloseReport>,
}
impl Client {
    pub fn connect(
        spec: Spec,
        info: Implementation,
        limits: Limits,
        parent: &TurnContext,
    ) -> Result<Self, Error> {
        Self::connect_in_context(spec, info, limits, parent, parent)
    }
    /// Keep the connection under parent while bounding only initialization by a
    /// shorter child context. Independent cancellation roots are rejected.
    pub fn connect_in_context(
        spec: Spec,
        info: Implementation,
        limits: Limits,
        parent: &TurnContext,
        handshake: &TurnContext,
    ) -> Result<Self, Error> {
        if !handshake.is_bounded_by(parent) {
            return Err(Error::Configuration);
        }
        handshake.remaining()?;
        parent.remaining()?;
        limits.validate()?;
        spec.validate(&limits)?;
        let session = ClientSession::new(info, limits.clone())?;
        let lifetime = parent.child(parent.remaining()?)?;
        parent.remaining()?;
        handshake.remaining()?;
        let mut process = Process::spawn(&spec)?;
        let input = process.child.stdin.take().ok_or(Error::Io)?;
        let output = process.child.stdout.take().ok_or(Error::Io)?;
        let error = process.child.stderr.take().ok_or(Error::Io)?;
        nonblocking(&input)?;
        nonblocking(&output)?;
        nonblocking(&error)?;
        let mut client = Self {
            thread_bound: std::marker::PhantomData,
            session,
            process,
            input: Some(input),
            output,
            error,
            lifetime,
            frames: Frames::new(limits.message_bytes, spec.queue_bytes),
            writes: VecDeque::new(),
            queued: 0,
            queue_limit: spec.queue_bytes,
            stdout_bytes: 0,
            stdout_limit: spec.stdout_limit,
            stderr_bytes: 0,
            stderr_limit: spec.stderr_limit,
            stderr_capture: spec.stderr_capture,
            stderr: VecDeque::new(),
            eof: false,
            grace: spec.shutdown_grace,
            outstanding: BTreeMap::new(),
            closed: None,
        };
        let init = client.session.initialize(handshake)?;
        client.enqueue_request(init)?;
        loop {
            match client.next(handshake)? {
                Incoming::Initialized(_) => {
                    let notification = client.session.initialized_notification()?;
                    client.enqueue(notification)?;
                    while !client.writes.is_empty() {
                        client.check(handshake)?;
                        if let Err(e) = client.pump() {
                            client.close();
                            return Err(e);
                        }
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    client.session.mark_initialized_sent()?;
                    break;
                }
                Incoming::Notification { .. } | Incoming::IgnoredLateReply => (),
                _ => {
                    client.close();
                    return Err(Error::Protocol(crate::Error::Phase));
                }
            }
        }
        Ok(client)
    }
    pub fn pid(&self) -> u32 {
        self.process.child.id()
    }
    pub fn server(&self) -> Option<&crate::types::InitializeResult> {
        self.session.server()
    }
    /// Explicit opt-in diagnostic access. No automatic stderr forwarding/logging.
    pub fn stderr_tail(&self) -> Vec<u8> {
        self.stderr.iter().copied().collect()
    }
    pub fn close_report(&self) -> Option<&CloseReport> {
        self.closed.as_ref()
    }
    fn check(&mut self, context: &TurnContext) -> Result<(), Error> {
        if self.closed.is_some() {
            return Err(Error::Closed);
        }
        if let Err(error) = self.lifetime.remaining().and_then(|_| context.remaining()) {
            self.close();
            return Err(Error::Context(error));
        }
        Ok(())
    }
    fn enqueue(&mut self, message: Message) -> Result<(), Error> {
        let mut bytes = self.session.encode(&message)?;
        bytes.push(b'\n');
        if self.writes.len() >= 64 || bytes.len() > self.queue_limit.saturating_sub(self.queued) {
            return Err(Error::Backpressure);
        }
        self.queued += bytes.len();
        self.writes.push_back(Outgoing { bytes, offset: 0 });
        Ok(())
    }
    fn enqueue_request(&mut self, message: Message) -> Result<Id, Error> {
        let Message::Request { id, method, .. } = &message else {
            return Err(Error::Protocol(crate::Error::Phase));
        };
        let id = id.clone();
        self.outstanding.insert(id.clone(), method.clone());
        if let Err(error) = self.enqueue(message) {
            self.close();
            return Err(error);
        }
        Ok(id)
    }
    pub fn send(
        &mut self,
        method: &str,
        params: Option<Map<String, Value>>,
        context: &TurnContext,
    ) -> Result<Id, Error> {
        self.check(context)?;
        let capped = context.child(context.remaining()?.min(self.lifetime.remaining()?))?;
        let message = self.session.request(method, params, &capped)?;
        self.enqueue_request(message)
    }
    /// Sequential convenience API. Use send/next for explicit pipelining.
    pub fn call(
        &mut self,
        method: &str,
        params: Option<Map<String, Value>>,
        context: &TurnContext,
    ) -> Result<Result<Map<String, Value>, crate::RpcError>, Error> {
        self.call_observed(method, params, context, |_| Ok(()))
    }
    /// Surface notifications during a sequential call. Observer errors close the
    /// generation and never cause a retry; callbacks must not execute host tools.
    pub fn call_observed(
        &mut self,
        method: &str,
        params: Option<Map<String, Value>>,
        context: &TurnContext,
        mut observer: impl FnMut(Incoming) -> Result<(), Error>,
    ) -> Result<Result<Map<String, Value>, crate::RpcError>, Error> {
        if !self.outstanding.is_empty() {
            return Err(Error::Busy);
        }
        let id = self.send(method, params, context)?;
        loop {
            match self.next(context)? {
                Incoming::Reply {
                    id: received,
                    result,
                    ..
                } if received == id => return Ok(result),
                notice @ Incoming::Notification { .. } => {
                    if let Err(error) = observer(notice) {
                        self.close();
                        return Err(error);
                    }
                }
                Incoming::IgnoredLateReply => (),
                _ => {
                    self.close();
                    return Err(Error::Protocol(crate::Error::ResponseId));
                }
            }
        }
    }
    /// This stdio generation shuts down on explicit cancellation, including other
    /// pending requests. A cancellation notification is best-effort, never replayed.
    pub fn cancel(&mut self, id: &Id) -> Result<bool, Error> {
        if self.closed.is_some() {
            return Err(Error::Closed);
        }
        let Some(notification) = self.session.cancel(id)? else {
            return Ok(false);
        };
        if self.writes.is_empty() {
            let _ = self.enqueue(notification);
            let _ = self.flush();
        }
        self.close();
        Ok(true)
    }
    pub fn next(&mut self, context: &TurnContext) -> Result<Incoming, Error> {
        loop {
            self.check(context)?;
            if let Some(expired) = self.session.expire().into_iter().next() {
                if self.writes.is_empty()
                    && let Some(notification) = expired.notification
                {
                    let _ = self.enqueue(notification);
                    let _ = self.flush();
                }
                self.close();
                return Err(Error::Context(expired.cause));
            }
            if let Some(frame) = self.frames.pop() {
                let incoming = match self.session.receive(&frame) {
                    Ok(v) => v,
                    Err(e) => {
                        self.close();
                        return Err(Error::Protocol(e));
                    }
                };
                match incoming {
                    Incoming::Respond(response) => {
                        if let Err(e) = self.enqueue(response) {
                            self.close();
                            return Err(e);
                        }
                        continue;
                    }
                    Incoming::Reply { ref id, .. } => {
                        self.outstanding.remove(id);
                        return Ok(incoming);
                    }
                    Incoming::Initialized(_) => {
                        self.outstanding.retain(|_, method| method != "initialize");
                        return Ok(incoming);
                    }
                    Incoming::Expired(expired) => {
                        self.close();
                        return Err(Error::Context(expired.cause));
                    }
                    other => return Ok(other),
                }
            }
            if self.eof {
                let error = if self.frames.partial() {
                    Error::Truncated
                } else {
                    Error::Eof
                };
                self.close();
                return Err(error);
            }
            if let Err(error) = self.pump() {
                self.close();
                return Err(error);
            }
            if !self.frames.ready() {
                match self.process.exited() {
                    Ok(true) => {
                        // The leader may have written its final reply between our
                        // first nonblocking read and waitid. Drain again before EOF.
                        let before = self.stdout_bytes;
                        if let Err(error) = self.pump() {
                            self.close();
                            return Err(error);
                        }
                        if self.stdout_bytes > before || self.frames.ready() {
                            continue;
                        }
                        let error = if self.frames.partial() {
                            Error::Truncated
                        } else {
                            Error::Eof
                        };
                        self.close();
                        return Err(error);
                    }
                    Err(error) => {
                        self.close();
                        return Err(error);
                    }
                    Ok(false) => (),
                }
            }
            if self.frames.partial() || !self.eof {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
    fn flush(&mut self) -> Result<(), Error> {
        for _ in 0..8 {
            let Some(front) = self.writes.front_mut() else {
                break;
            };
            let input = self.input.as_mut().ok_or(Error::Closed)?;
            let end = (front.offset + 4096).min(front.bytes.len());
            match input.write(&front.bytes[front.offset..end]) {
                Ok(0) => return Err(Error::Io),
                Ok(n) => {
                    front.offset += n;
                    self.queued -= n;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(Error::Io),
            }
            if front.offset == front.bytes.len() {
                self.writes.pop_front();
            }
        }
        Ok(())
    }
    fn pump(&mut self) -> Result<(), Error> {
        self.flush()?;
        for _ in 0..8 {
            let mut b = [0u8; 4096];
            match self.output.read(&mut b) {
                Ok(0) => {
                    self.eof = true;
                    break;
                }
                Ok(n) => {
                    self.stdout_bytes += n;
                    if self.stdout_bytes > self.stdout_limit {
                        return Err(Error::OutputLimit);
                    }
                    self.frames.push(&b[..n])?;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(Error::Io),
            }
        }
        self.drain_stderr()?;
        self.flush()
    }
    fn drain_stderr(&mut self) -> Result<(), Error> {
        for _ in 0..8 {
            let mut b = [0u8; 4096];
            match self.error.read(&mut b) {
                Ok(0) => break,
                Ok(n) => {
                    self.stderr_bytes += n;
                    for byte in &b[..n] {
                        if self.stderr_capture > 0 {
                            if self.stderr.len() == self.stderr_capture {
                                self.stderr.pop_front();
                            }
                            self.stderr.push_back(*byte);
                        }
                    }
                    if self.stderr_bytes > self.stderr_limit {
                        return Err(Error::OutputLimit);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(Error::Io),
            }
        }
        Ok(())
    }
    pub fn close(&mut self) -> CloseReport {
        if let Some(report) = &self.closed {
            return report.clone();
        }
        self.input.take();
        let _ = self.session.close();
        let forced = self.process.stop(self.grace);
        let _ = self.drain_stderr();
        self.writes.clear();
        self.queued = 0;
        let abandoned = std::mem::take(&mut self.outstanding)
            .into_iter()
            .map(|(id, method)| Abandoned { id, method })
            .collect();
        let report = CloseReport {
            exit_code: self.process.status.and_then(|s| s.code()),
            forced,
            abandoned,
            stderr_bytes: self.stderr_bytes,
            stderr_truncated: self.stderr_bytes > self.stderr.len(),
        };
        self.closed = Some(report.clone());
        report
    }
    pub fn phase(&self) -> Phase {
        self.session.phase()
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.close();
    }
}
