//! Explicit single-worker authenticated loopback AG-UI host. No remote scope selection.
mod http;
use crate::execute::{RunOptions, config_for, execute_with_source};
use clap::Args as ClapArgs;
use namzu_ag_ui::{Mapper, RunInput, WireEvent};
use namzu_core::{AgentError, Cancellation, Event, EventSink, ToolCall, TurnContext};
use namzu_session::{PrivateStore, default_state_dir, new_id};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    io,
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{Arc, Mutex, mpsc},
    thread::JoinHandle,
    time::{Duration, Instant},
};
#[derive(ClapArgs)]
pub struct Args {
    /// Bind loopback only. Port 0 chooses a free local port.
    #[arg(long, default_value = "127.0.0.1:0")]
    listen: SocketAddr,
    /// Required environment variable NAME holding a 32..512-byte bearer capability.
    #[arg(long, default_value = "NAMZU_AGUI_TOKEN")]
    auth_token_env: String,
    #[command(flatten)]
    run: RunOptions,
}
fn hash(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))
}
fn failed() -> AgentError {
    AgentError::State("AG-UI private thread state is invalid or unavailable")
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    version: u32,
    session: Option<String>,
    phase: Phase,
    runs: Vec<String>,
    messages: Vec<String>,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Ready,
    Running,
    Blocked,
}
impl Binding {
    fn validate(&self) -> Result<(), AgentError> {
        if self.version != 1
            || self.runs.len() > 1024
            || self.messages.len() > 1024
            || self
                .runs
                .iter()
                .chain(self.messages.iter())
                .any(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            Err(failed())
        } else {
            Ok(())
        }
    }
}
struct Pending {
    id: String,
    reply: mpsc::SyncSender<bool>,
}
struct Control {
    thread: String,
    run: String,
    cancel: Cancellation,
    pending: Option<Pending>,
}
type Shared = Arc<Mutex<Option<Control>>>;
struct Active {
    control: Shared,
    monitor: TcpStream,
    worker: JoinHandle<()>,
}
struct Sink {
    socket: TcpStream,
    mapper: Mapper,
    control: Shared,
    context: TurnContext,
}
impl Sink {
    fn send(&mut self, events: Vec<WireEvent>) -> Result<(), AgentError> {
        for event in events {
            self.context.remaining()?;
            let bytes = namzu_ag_ui::sse(&event)?;
            let deadline = Instant::now() + self.context.remaining()?.min(Duration::from_secs(2));
            http::write_until(&mut self.socket, &bytes, deadline, || {
                self.context.remaining().is_ok()
            })
            .map_err(|_| {
                self.context.cancellation().cancel();
                AgentError::Cancelled
            })?;
        }
        Ok(())
    }
    fn proposal(&mut self, call: &ToolCall) -> Result<(), AgentError> {
        let events = self.mapper.propose(call)?;
        self.send(events)
    }
}
fn read_only(name: &str) -> bool {
    matches!(
        name,
        "read_file"
            | "list_files"
            | "inspect_file"
            | "read_file_chunk"
            | "read_document"
            | "read_image"
            | "search_files"
            | "diff_file"
            | "computer_info"
            | "computer_screenshot"
            | "browser_info"
            | "browser_snapshot"
    )
}
impl EventSink for Sink {
    fn supports_approval(&self) -> bool {
        true
    }
    fn tool_proposed(&mut self, c: &ToolCall) -> Result<(), AgentError> {
        self.proposal(c)
    }
    fn emit(&mut self, event: Event) -> Result<(), AgentError> {
        let events = self.mapper.map(&event)?;
        self.send(events)
    }
    fn approve(&mut self, call: &ToolCall, ctx: &TurnContext) -> Result<bool, AgentError> {
        ctx.remaining()?;
        if read_only(&call.name) {
            return Ok(true);
        }
        let id = format!("a-{}", hash(&new_id()));
        let (tx, rx) = mpsc::sync_channel(1);
        {
            let mut guard = self.control.lock().map_err(|_| failed())?;
            let current = guard.as_mut().ok_or(AgentError::Cancelled)?;
            if current.pending.is_some() {
                return Err(failed());
            }
            current.pending = Some(Pending {
                id: id.clone(),
                reply: tx,
            });
        }
        let outcome = (|| {
            let events = self.mapper.approval(&id, call)?;
            self.send(events)?;
            loop {
                ctx.remaining()?;
                match rx.recv_timeout(Duration::from_millis(10)) {
                    Ok(allowed) => {
                        ctx.remaining()?;
                        return Ok(allowed);
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(_) => return Err(AgentError::Cancelled),
                }
            }
        })();
        if let Ok(mut guard) = self.control.lock()
            && let Some(c) = guard.as_mut()
        {
            c.pending = None;
        }
        outcome
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Decision {
    thread_id: String,
    run_id: String,
    approval_id: String,
    decision: Choice,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Choice {
    Allow,
    Deny,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Cancel {
    thread_id: String,
    run_id: String,
}
fn equal_token(a: &str, b: &str) -> bool {
    let mut d = a.len() ^ b.len();
    for (i, v) in b.bytes().enumerate() {
        d |= usize::from(a.as_bytes().get(i).copied().unwrap_or(0) ^ v);
    }
    d == 0
}
fn valid_token(s: &str) -> bool {
    (32..=512).contains(&s.len()) && s.bytes().all(|b| (33..=126).contains(&b))
}
pub fn run(mut args: Args, cancel: Cancellation) -> Result<(), AgentError> {
    if !args.listen.ip().is_loopback() || args.run.resume.is_some() || args.run.mcp_approve {
        return Err(AgentError::Configuration(
            "AG-UI requires loopback, server-owned history and per-call MCP approval",
        ));
    }
    let token = std::env::var(&args.auth_token_env).map_err(|_| {
        AgentError::Configuration("AG-UI bearer token environment variable required")
    })?;
    if !valid_token(&token) {
        return Err(AgentError::Configuration(
            "AG-UI bearer token must be 32..512 visible ASCII bytes",
        ));
    }
    let state = args
        .run
        .state_dir
        .clone()
        .map_or_else(default_state_dir, Ok)?;
    args.run.state_dir = Some(state.clone());
    // Keep every possible settled result below the single-event wire bound.
    if args.run.max_text_bytes.is_some_and(|n| n > 128 * 1024) {
        return Err(AgentError::Configuration(
            "AG-UI conversation budget cannot exceed 128 KiB",
        ));
    }
    let store = PrivateStore::open(&state)?;
    let c = config_for(&args.run, &store)?;
    // Resolve explicit config/CLI limits first; the service cap must never raise
    // a host's smaller budget just because no CLI override was supplied.
    args.run.max_text_bytes = Some(c.max_text_bytes.min(128 * 1024));
    if c.timeout > 300 || c.model.is_none() {
        return Err(AgentError::Configuration(
            "AG-UI requires an explicit model and timeout <= 300 seconds",
        ));
    }
    let bindings = store.child("agui")?;
    let listener = TcpListener::bind(args.listen)
        .map_err(|_| AgentError::State("cannot bind AG-UI listener"))?;
    listener.set_nonblocking(true).map_err(|_| failed())?;
    let address = listener.local_addr().map_err(|_| failed())?;
    println!(
        "{}",
        json!({"format_version":1,"status":"listening","url":format!("http://{address}/agent"),"auth":"bearer","max_active_runs":1})
    );
    let mut connections = vec![];
    let mut active: Option<Active> = None;
    while !cancel.is_cancelled() {
        if let Some(a) = &active {
            let mut b = [0];
            match a.monitor.peek(&mut b) {
                Ok(0) => {
                    if let Ok(g) = a.control.lock()
                        && let Some(c) = g.as_ref()
                    {
                        c.cancel.cancel();
                    }
                }
                Err(e) if e.kind() != io::ErrorKind::WouldBlock => {
                    if let Ok(g) = a.control.lock()
                        && let Some(c) = g.as_ref()
                    {
                        c.cancel.cancel();
                    }
                }
                _ => {}
            }
        }
        if active.as_ref().is_some_and(|a| a.worker.is_finished()) {
            let a = active.take().unwrap();
            a.worker.join().map_err(|_| {
                AgentError::State("AG-UI worker stopped; inspect pending thread/session")
            })?;
        }
        for _ in 0..16 {
            match listener.accept() {
                Ok((mut socket, _)) => {
                    socket.set_nonblocking(true).map_err(|_| failed())?;
                    if connections.len() >= 16 {
                        http::response(&mut socket, 503, "NAMZU_HTTP_BUSY");
                    } else {
                        connections.push(http::Connection::new(socket));
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => return Err(failed()),
            }
        }
        let mut index = 0;
        while index < connections.len() {
            match connections[index].poll() {
                Ok(None) => {
                    index += 1;
                    continue;
                }
                Err(status) => {
                    let mut conn = connections.swap_remove(index);
                    http::response(&mut conn.socket, status, "NAMZU_HTTP_REQUEST");
                }
                Ok(Some(request)) => {
                    let mut conn = connections.swap_remove(index);
                    if request.header("origin").is_some() {
                        http::response(&mut conn.socket, 403, "NAMZU_ORIGIN_DISABLED");
                        continue;
                    }
                    let host = request.header("host");
                    if host != Some(address.to_string().as_str()) {
                        http::response(&mut conn.socket, 400, "NAMZU_HOST");
                        continue;
                    }
                    if !request
                        .header("authorization")
                        .and_then(|v| v.strip_prefix("Bearer "))
                        .is_some_and(|v| equal_token(v, &token))
                    {
                        http::response(&mut conn.socket, 401, "NAMZU_UNAUTHORIZED");
                        continue;
                    }
                    if request.method == "GET" && request.path == "/health" {
                        http::response(&mut conn.socket, 200, "NAMZU_READY");
                        continue;
                    }
                    if request.method != "POST" {
                        http::response(&mut conn.socket, 405, "NAMZU_METHOD");
                        continue;
                    }
                    if request
                        .header("content-type")
                        .is_none_or(|s| s.split(';').next() != Some("application/json"))
                    {
                        http::response(&mut conn.socket, 415, "NAMZU_CONTENT_TYPE");
                        continue;
                    }
                    if matches!(request.path.as_str(), "/cancel" | "/approve") {
                        let status = control_request(&request, &active);
                        http::response(
                            &mut conn.socket,
                            status,
                            if status == 202 {
                                "NAMZU_ACCEPTED"
                            } else {
                                "NAMZU_NO_MATCHING_CONTROL"
                            },
                        );
                        continue;
                    }
                    if request.path != "/agent" {
                        http::response(&mut conn.socket, 404, "NAMZU_NOT_FOUND");
                        continue;
                    }
                    if active.is_some() {
                        http::response(&mut conn.socket, 409, "NAMZU_TURN_IN_PROGRESS");
                        continue;
                    }
                    let input = match serde_json::from_slice::<RunInput>(&request.body) {
                        Ok(v) if v.admit().is_ok() => v,
                        _ => {
                            http::response(&mut conn.socket, 400, "NAMZU_INPUT_SCOPE");
                            continue;
                        }
                    };
                    let (message, prompt) = input.admit()?;
                    let prompt = prompt.to_owned();
                    let message = hash(message);
                    let tag = hash(&format!("{token}\0{}", input.thread_id));
                    let reserved = (|| {
                        // One fixed admission lock bounds the index across server processes.
                        // Never create per-client lock files before checking storage capacity.
                        let _lock = bindings.lock("admission")?;
                        let names = bindings.names()?;
                        let filename = format!("{tag}.json");
                        if !names.contains(&filename)
                            && names.iter().filter(|n| n.ends_with(".json")).count() >= 4096
                        {
                            return Err(failed());
                        }
                        let mut binding = bindings
                            .read::<Binding>(&format!("{tag}.json"))?
                            .unwrap_or(Binding {
                                version: 1,
                                session: None,
                                phase: Phase::Ready,
                                runs: vec![],
                                messages: vec![],
                            });
                        binding.validate()?;
                        if binding.phase != Phase::Ready
                            || binding.runs.contains(&hash(&input.run_id))
                            || binding.messages.contains(&message)
                            || binding.runs.len() >= 1024
                        {
                            return Err(failed());
                        }
                        binding.runs.push(hash(&input.run_id));
                        binding.messages.push(message);
                        binding.phase = Phase::Running;
                        bindings.write(&format!("{tag}.json"), &binding)?;
                        Ok(binding)
                    })();
                    let binding = match reserved {
                        Ok(v) => v,
                        Err(_) => {
                            http::response(
                                &mut conn.socket,
                                409,
                                "NAMZU_THREAD_BLOCKED_OR_REPLAYED",
                            );
                            continue;
                        }
                    };
                    let control = Arc::new(Mutex::new(Some(Control {
                        thread: input.thread_id.clone(),
                        run: input.run_id.clone(),
                        cancel: cancel.child(),
                        pending: None,
                    })));
                    let monitor = conn.socket.try_clone().map_err(|_| failed())?;
                    let mut options = args.run.clone();
                    options.resume = binding.session.clone();
                    let control_worker = control.clone();
                    let thread_store = PrivateStore::open(&state)?.open_child("agui")?;
                    let timeout = c.timeout;
                    let worker = std::thread::spawn(move || {
                        let cancellation = control_worker
                            .lock()
                            .unwrap()
                            .as_ref()
                            .unwrap()
                            .cancel
                            .clone();
                        let context =
                            TurnContext::new(Duration::from_secs(timeout), cancellation.clone())
                                .unwrap();
                        let mut sink = Sink {
                            socket: conn.socket,
                            mapper: Mapper::new(&input.thread_id, &input.run_id).unwrap(),
                            control: control_worker.clone(),
                            context,
                        };
                        let headers=b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\r\n";
                        let opened = http::write_until(
                            &mut sink.socket,
                            headers,
                            Instant::now() + Duration::from_secs(2),
                            || !cancellation.is_cancelled(),
                        )
                        .is_ok();
                        let start = sink.mapper.start().and_then(|v| sink.send(v));
                        if !opened || start.is_err() {
                            cancellation.cancel();
                        }
                        let mut execution = if opened && start.is_ok() {
                            execute_with_source(
                                prompt,
                                options,
                                cancellation,
                                &mut sink,
                                namzu_telemetry::Source::Agui,
                            )
                        } else {
                            crate::execute::Execution {
                                session_id: binding.session.clone(),
                                telemetry_id: None,
                                result: Err(AgentError::Cancelled),
                            }
                        };
                        let mut final_binding = binding;
                        final_binding.phase = if execution.result.is_ok() {
                            Phase::Ready
                        } else {
                            Phase::Blocked
                        };
                        final_binding.session = execution.session_id.clone();
                        if thread_store
                            .write(&format!("{tag}.json"), &final_binding)
                            .is_err()
                        {
                            execution.result = Err(failed());
                        }
                        let events = sink.mapper.finish(
                            &execution.result,
                            execution.session_id.as_deref(),
                            execution.telemetry_id.as_deref(),
                        );
                        for event in events {
                            if let Ok(bytes) = namzu_ag_ui::sse(&event) {
                                let _ = http::write_until(
                                    &mut sink.socket,
                                    &bytes,
                                    Instant::now() + Duration::from_secs(1),
                                    || true,
                                );
                            }
                        }
                        if let Ok(mut g) = control_worker.lock() {
                            *g = None;
                        }
                    });
                    active = Some(Active {
                        control,
                        monitor,
                        worker,
                    });
                }
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    if let Some(a) = active {
        if let Ok(g) = a.control.lock()
            && let Some(c) = g.as_ref()
        {
            c.cancel.cancel();
        }
        a.worker.join().map_err(|_| failed())?;
    }
    Ok(())
}
fn control_request(request: &http::Request, active: &Option<Active>) -> u16 {
    let Some(active) = active else {
        return 404;
    };
    let Ok(mut guard) = active.control.lock() else {
        return 409;
    };
    let Some(current) = guard.as_mut() else {
        return 404;
    };
    if request.path == "/cancel" {
        let Ok(c) = serde_json::from_slice::<Cancel>(&request.body) else {
            return 400;
        };
        if c.thread_id != current.thread || c.run_id != current.run {
            return 409;
        }
        current.cancel.cancel();
        return 202;
    }
    let Ok(d) = serde_json::from_slice::<Decision>(&request.body) else {
        return 400;
    };
    if d.thread_id != current.thread || d.run_id != current.run || current.cancel.is_cancelled() {
        return 409;
    }
    let Some(pending) = current.pending.take() else {
        return 409;
    };
    if d.approval_id != pending.id {
        current.pending = Some(pending);
        return 409;
    }
    if pending
        .reply
        .try_send(matches!(d.decision, Choice::Allow))
        .is_ok()
    {
        202
    } else {
        409
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capability_tokens_are_not_partial_or_empty_matches() {
        assert!(equal_token("abc", "abc"));
        for v in ["", "ab", "abcd", "xbc"] {
            assert!(!equal_token(v, "abc"));
        }
        assert!(!valid_token("short"));
        assert!(!valid_token(&"x\n".repeat(20)));
    }
}
