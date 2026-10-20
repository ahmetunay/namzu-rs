//! One behavioural contract across every implemented service/wire/auth profile.
//! Only loopback, fake credentials, and independent tool/file oracles.
use crate::{
    OpenAiCompatible,
    codex::CodexProvider,
    services::{Format, Options, Service},
};
use namzu_core::{
    provider::{FailureKind, Streaming},
    *,
};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug)]
struct Case {
    service: Option<Service>,
    format: Format,
    stream: bool,
    sigv4: bool,
}
fn cases() -> Vec<Case> {
    let mut result = vec![];
    for service in [
        Service::OpenaiCompatible,
        Service::Openrouter,
        Service::Zen,
        Service::ZenGo,
        Service::Deepseek,
        Service::Lmstudio,
    ] {
        for stream in [false, true] {
            result.push(Case {
                service: Some(service),
                format: Format::Chat,
                stream,
                sigv4: false,
            });
        }
    }
    for service in [Service::OpenaiCompatible, Service::Zen, Service::ZenGo] {
        for stream in [false, true] {
            result.push(Case {
                service: Some(service),
                format: Format::Responses,
                stream,
                sigv4: false,
            });
        }
    }
    for (service, format) in [
        (Service::Anthropic, Format::Messages),
        (Service::Google, Format::Google),
        (Service::Bedrock, Format::Converse),
        (Service::Ollama, Format::Ollama),
        (Service::Zen, Format::Messages),
        (Service::Zen, Format::Google),
        (Service::ZenGo, Format::Messages),
        (Service::ZenGo, Format::Google),
    ] {
        result.push(Case {
            service: Some(service),
            format,
            stream: false,
            sigv4: false,
        });
    }
    result.push(Case {
        service: Some(Service::Bedrock),
        format: Format::Converse,
        stream: false,
        sigv4: true,
    });
    result.push(Case {
        service: None,
        format: Format::Responses,
        stream: true,
        sigv4: false,
    });
    assert_eq!(result.len(), 28);
    result
}
impl Case {
    fn model(self) -> &'static str {
        match self.format {
            Format::Messages => "claude-fixture",
            Format::Google => "gemini-fixture",
            _ => "fixture",
        }
    }
    fn provider(self, url: &str, state: PathBuf) -> Driver {
        let Some(service) = self.service else {
            return Driver::Codex(CodexProvider::conformance_fixture(state, url, self.model()));
        };
        let options = Options {
            format: self.format,
            stream: self.stream,
            max_output_tokens: Some(128),
            bedrock_auth: self.sigv4.then(|| "sigv4".into()),
            aws_region: self.sigv4.then(|| "us-east-1".into()),
            ..Default::default()
        };
        let key = if service.key_optional() || self.sigv4 {
            ""
        } else {
            "conformance-fake-key"
        };
        // SigV4 constructor requires a placeholder bearer, replaced by explicit fake credentials.
        let mut p = OpenAiCompatible::for_service(
            service,
            url,
            self.model().into(),
            if self.sigv4 { "fixture" } else { key },
            options,
        )
        .unwrap();
        if self.sigv4 {
            p.aws = Some(
                crate::aws::Credentials::new(
                    "us-east-1".into(),
                    "FIXTUREACCESS".into(),
                    "fixture-secret".into(),
                    Some("fixture-token".into()),
                )
                .unwrap(),
            );
        }
        p.client = crate::http::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        Driver::Api(p)
    }
    fn path(self) -> String {
        match self.format {
            Format::Messages => "/messages".into(),
            Format::Google => format!("/models/{}:generateContent", self.model()),
            Format::Converse => format!("/model/{}/converse", self.model()),
            Format::Ollama => "/api/chat".into(),
            Format::Responses => "/responses".into(),
            _ => "/chat/completions".into(),
        }
    }
    fn verify(self, request: &Request) {
        assert_eq!(request.path, self.path(), "{self:?}");
        let key = "conformance-fake-key";
        let header = |name: &str| {
            request.headers.lines().find_map(|l| {
                l.split_once(':')
                    .filter(|(k, _)| k.eq_ignore_ascii_case(name))
                    .map(|(_, v)| v.trim())
            })
        };
        if self.service.is_none() {
            assert!(
                header("authorization")
                    .unwrap()
                    .starts_with("Bearer fixture.")
            );
            assert_eq!(header("chatgpt-account-id"), Some("conformance"));
            assert_eq!(request.body["store"], false);
        } else if self.sigv4 {
            assert!(
                header("authorization")
                    .unwrap()
                    .starts_with("AWS4-HMAC-SHA256 ")
            );
            assert_eq!(header("x-amz-security-token"), Some("fixture-token"));
        } else if self.service.is_some_and(Service::key_optional) {
            assert!(header("authorization").is_none());
        } else if self.format == Format::Messages {
            assert_eq!(header("x-api-key"), Some(key));
            assert!(header("authorization").is_none());
        } else if self.format == Format::Google {
            assert_eq!(header("x-goog-api-key"), Some(key));
            assert!(header("authorization").is_none());
        } else {
            assert_eq!(header("authorization"), Some("Bearer conformance-fake-key"));
        }
        if !matches!(self.format, Format::Google | Format::Converse) {
            assert_eq!(request.body["model"], self.model());
        }
    }
}
// Sized adapter keeps the production core API unchanged.
enum Driver {
    Api(OpenAiCompatible),
    Codex(CodexProvider),
}
impl Provider for Driver {
    fn info(&self) -> ProviderInfo {
        match self {
            Self::Api(p) => p.info(),
            Self::Codex(p) => p.info(),
        }
    }
    fn take_metadata(&mut self) -> Option<ResponseMetadata> {
        match self {
            Self::Api(p) => p.take_metadata(),
            Self::Codex(p) => p.take_metadata(),
        }
    }
    fn take_state(&mut self) -> Option<Value> {
        match self {
            Self::Api(p) => p.take_state(),
            Self::Codex(p) => p.take_state(),
        }
    }
    fn complete(
        &mut self,
        m: &[Message],
        t: &[ToolDefinition],
        c: &TurnContext,
    ) -> Result<Completion, AgentError> {
        self.complete_stream(m, t, c, &mut NoopEvents)
    }
    fn complete_stream(
        &mut self,
        m: &[Message],
        t: &[ToolDefinition],
        c: &TurnContext,
        e: &mut dyn EventSink,
    ) -> Result<Completion, AgentError> {
        match self {
            Self::Api(p) => p.complete_stream(m, t, c, e),
            Self::Codex(p) => p.complete_stream(m, t, c, e),
        }
    }
}
#[derive(Clone)]
struct Request {
    path: String,
    headers: String,
    body: Value,
}
struct Reply {
    status: u16,
    headers: String,
    body: String,
    extra: usize,
}
impl Reply {
    fn ok(body: String) -> Self {
        Self {
            status: 200,
            headers: String::new(),
            body,
            extra: 0,
        }
    }
}
struct Fixture {
    url: String,
    requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
fn read_request(stream: &mut TcpStream) -> Request {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut bytes = vec![];
    while !bytes.ends_with(b"\r\n\r\n") {
        let mut b = [0];
        stream.read_exact(&mut b).unwrap();
        bytes.push(b[0]);
        assert!(bytes.len() < 32768);
    }
    let headers = String::from_utf8(bytes).unwrap();
    let size = headers
        .lines()
        .find_map(|l| {
            l.split_once(':')
                .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                .map(|(_, v)| v.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    assert!(size <= 8 * 1024 * 1024);
    let mut bytes = vec![0; size];
    stream.read_exact(&mut bytes).unwrap();
    Request {
        path: headers
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .into(),
        headers,
        body: serde_json::from_slice(&bytes).unwrap(),
    }
}

fn accept_bounded(listener: TcpListener) -> TcpStream {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match listener.accept() {
            Ok((stream, _)) => return stream,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "expected in-flight fixture request"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(e) => panic!("{e}"),
        }
    }
}
impl Fixture {
    fn new(mut handler: impl FnMut(&Request, usize) -> Reply + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(vec![]));
        let received = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = std::thread::spawn(move || {
            while !stopped.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let request = read_request(&mut stream);
                        let index = received.lock().unwrap().len();
                        let reply = handler(&request, index);
                        received.lock().unwrap().push(request);
                        let _ = write!(
                            stream,
                            "HTTP/1.1 {} Fixture\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{}\r\n{}",
                            reply.status,
                            reply.body.len() + reply.extra,
                            reply.headers,
                            reply.body
                        );
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1))
                    }
                    Err(e) => panic!("{e}"),
                }
            }
        });
        Self {
            url,
            requests,
            stop,
            worker: Some(worker),
        }
    }
    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(w) = self.worker.take() {
            let result = w.join();
            if !std::thread::panicking() {
                result.unwrap();
            }
        }
    }
}
fn context() -> TurnContext {
    TurnContext::new(Duration::from_secs(10), Cancellation::default()).unwrap()
}
fn user() -> Vec<Message> {
    vec![Message::User {
        content: "fixture".into(),
    }]
}
fn event(v: Value) -> String {
    format!("data: {v}\n\n")
}
#[derive(Clone, Copy)]
enum Usage {
    Known,
    Absent,
    Invalid,
}
fn wire(case: Case, call: bool, usage: Usage, incomplete: bool, malformed: bool) -> String {
    let args = if malformed {
        json!([])
    } else {
        json!({"value":42})
    };
    let text = if call { "" } else { "CONFORMANCE_OK" };
    let mut v = match case.format {
        Format::Chat => {
            json!({"choices":[{"finish_reason":if incomplete{"length"}else if call{"tool_calls"}else{"stop"},"message":{"role":"assistant","content":text,"reasoning_content":"PRIVATE_REASONING"}}]})
        }
        Format::Responses => {
            json!({"status":if incomplete{"incomplete"}else{"completed"},"output":[if call{json!({"type":"function_call","id":"item","call_id":"call","name":"write","arguments":args.to_string()})}else{json!({"type":"message","id":"m","role":"assistant","status":"completed","content":[{"type":"output_text","text":text}]})}]})
        }
        Format::Messages => {
            json!({"type":"message","role":"assistant","stop_reason":if incomplete{"max_tokens"}else if call{"tool_use"}else{"end_turn"},"content":[if call{json!({"type":"tool_use","id":"call","name":"write","input":args})}else{json!({"type":"text","text":text})}]})
        }
        Format::Google => {
            json!({"candidates":[{"finishReason":if incomplete{"MAX_TOKENS"}else{"STOP"},"content":{"role":"model","parts":[if call{json!({"functionCall":{"name":"write","args":args},"thoughtSignature":"PRIVATE_SIGNATURE"})}else{json!({"text":text})}]}}]})
        }
        Format::Converse => {
            json!({"stopReason":if incomplete{"max_tokens"}else if call{"tool_use"}else{"end_turn"},"output":{"message":{"role":"assistant","content":[if call{json!({"toolUse":{"toolUseId":"call","name":"write","input":args}})}else{json!({"text":text})}]}}})
        }
        Format::Ollama => {
            json!({"done":!incomplete,"done_reason":"stop","message":{"role":"assistant","content":text,"tool_calls":if call{json!([{"function":{"name":"write","arguments":args}}])}else{json!([])}}})
        }
        _ => unreachable!(),
    };
    if case.format == Format::Chat && call {
        v["choices"][0]["message"]["tool_calls"] = json!([{"id":"call","type":"function","function":{"name":"write","arguments":args.to_string()}}]);
    }
    if !matches!(usage, Usage::Absent) {
        let input = if matches!(usage, Usage::Invalid) {
            json!(-1)
        } else {
            json!(3)
        };
        match case.format {
            Format::Chat => {
                v["usage"] = json!({"prompt_tokens":input,"completion_tokens":2,"total_tokens":5})
            }
            Format::Responses | Format::Messages => {
                v["usage"] = json!({"input_tokens":input,"output_tokens":2,"total_tokens":5})
            }
            Format::Google => {
                v["usageMetadata"] =
                    json!({"promptTokenCount":input,"candidatesTokenCount":2,"totalTokenCount":5})
            }
            Format::Converse => {
                v["usage"] = json!({"inputTokens":input,"outputTokens":2,"totalTokens":5})
            }
            Format::Ollama => {
                v["prompt_eval_count"] = input;
                v["eval_count"] = json!(2);
            }
            _ => unreachable!(),
        }
    }
    if !case.stream {
        return v.to_string();
    }
    if case.format == Format::Responses {
        let mut result = event(json!({"type":"response.output_text.delta","delta":text}));
        result += &event(json!({"type":"response.completed","response":v}));
        return result;
    }
    let m = &v["choices"][0]["message"];
    let mut delta =
        json!({"role":"assistant","content":text,"reasoning_content":"PRIVATE_REASONING"});
    if call {
        let mut c = m["tool_calls"][0].clone();
        c["index"] = json!(0);
        delta["tool_calls"] = json!([c]);
    }
    let mut result = event(
        json!({"choices":[{"index":0,"delta":delta,"finish_reason":v["choices"][0]["finish_reason"]}],"usage":v["usage"]}),
    );
    result += "data: [DONE]\n\n";
    result
}
#[derive(Default)]
struct Events(Vec<Event>);
impl EventSink for Events {
    fn emit(&mut self, e: Event) -> Result<(), AgentError> {
        self.0.push(e);
        Ok(())
    }
}
struct Oracle {
    path: PathBuf,
    calls: usize,
    fail: bool,
}
impl Tools for Oracle {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "write".into(),
            description: "fixture oracle".into(),
            parameters: json!({"type":"object","properties":{"value":{"type":"integer"}},"required":["value"],"additionalProperties":false}),
        }]
    }
    fn call(&mut self, c: &ToolCall, _: &TurnContext) -> Result<ToolResult, AgentError> {
        assert_eq!(c.name, "write");
        assert_eq!(c.arguments, json!({"value":42}));
        self.calls += 1;
        if self.fail {
            return Ok(ToolResult::error("TOOL_DENIED"));
        }
        std::fs::write(&self.path, b"42\n").unwrap();
        Ok(ToolResult::ok("ORACLE_WRITTEN"))
    }
}
fn oracle(root: &std::path::Path, fail: bool) -> Oracle {
    Oracle {
        path: root.join("answer"),
        calls: 0,
        fail,
    }
}
fn limits() -> Limits {
    Limits {
        timeout: Duration::from_secs(10),
        max_steps: 3,
        max_tool_calls: 2,
        ..Default::default()
    }
}

fn nested<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Object(map) => map
            .get(key)
            .or_else(|| map.values().find_map(|v| nested(v, key))),
        Value::Array(values) => values.iter().find_map(|v| nested(v, key)),
        _ => None,
    }
}

fn verify_tool_result(case: Case, body: &Value, failed: bool) {
    let expected = if failed {
        "TOOL_DENIED"
    } else {
        "ORACLE_WRITTEN"
    };
    match case.format {
        Format::Chat | Format::Ollama => {
            let result = body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["role"] == "tool")
                .unwrap();
            let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
            assert_eq!(content["is_error"], failed);
            assert_eq!(content["content"], expected);
            if case.format == Format::Chat {
                assert_eq!(result["tool_call_id"], "call");
            } else {
                assert_eq!(result["tool_name"], "write");
            }
        }
        Format::Responses => {
            let result = body["input"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["type"] == "function_call_output")
                .unwrap();
            assert_eq!(result["call_id"], "call");
            let content: Value = serde_json::from_str(result["output"].as_str().unwrap()).unwrap();
            assert_eq!(content["is_error"], failed);
            assert_eq!(content["content"], expected);
        }
        Format::Messages => {
            assert_eq!(nested(body, "tool_use_id").unwrap(), "call");
            assert_eq!(nested(body, "is_error").unwrap(), failed);
        }
        Format::Google => {
            let result = nested(body, "functionResponse").unwrap();
            assert_eq!(result["name"], "write");
            assert!(result.get("id").is_none());
            assert_eq!(
                result["response"][if failed { "error" } else { "output" }],
                expected
            );
        }
        Format::Converse => {
            let result = nested(body, "toolResult").unwrap();
            assert_eq!(result["toolUseId"], "call");
            assert_eq!(result["status"], if failed { "error" } else { "success" });
            assert_eq!(result["content"][0]["text"], expected);
        }
        _ => unreachable!(),
    }
}

#[test]
fn matrix_text_descriptor_usage_events_and_one_shot_reset() {
    for case in cases() {
        eprintln!("{case:?}");
        let root = tempfile::tempdir().unwrap();
        let server = Fixture::new(move |r, _| {
            case.verify(r);
            Reply::ok(wire(case, false, Usage::Known, false, false))
        });
        let mut p = case.provider(&server.url, root.path().into());
        let info = p.info();
        info.validate().unwrap();
        assert_eq!(info.model.as_deref(), Some(case.model()));
        assert_eq!(
            info.capabilities.streaming,
            if case.stream {
                Streaming::Incremental
            } else {
                Streaming::Buffered
            }
        );
        let mut events = Events::default();
        let completion = p
            .complete_stream(&user(), &[], &context(), &mut events)
            .unwrap();
        assert_eq!(completion.text, "CONFORMANCE_OK");
        assert!(completion.tool_calls.is_empty());
        let metadata = p.take_metadata().unwrap();
        assert_eq!(
            metadata.usage.unwrap(),
            TokenUsage {
                input_tokens: Some(3),
                output_tokens: Some(2),
                total_tokens: Some(5),
                ..Default::default()
            }
        );
        assert!(metadata.cost.is_none());
        assert!(p.take_metadata().is_none());
        let deltas = events
            .0
            .iter()
            .map(|e| match e {
                Event::AssistantDelta { text } => text.as_str(),
                _ => "",
            })
            .collect::<String>();
        assert_eq!(deltas, "CONFORMANCE_OK");
        assert!(!format!("{:?}", events.0).contains("PRIVATE_"));
        p.complete(&user(), &[], &context()).unwrap();
        let cancel = Cancellation::default();
        cancel.cancel();
        assert_eq!(
            p.complete(
                &user(),
                &[],
                &TurnContext::new(Duration::from_secs(10), cancel).unwrap()
            )
            .unwrap_err(),
            AgentError::Cancelled
        );
        assert!(p.take_metadata().is_none());
        assert!(p.take_state().is_none());
        assert_eq!(server.requests().len(), 2);
    }
}
#[test]
fn matrix_absent_usage_and_cost_stay_unknown() {
    for case in cases() {
        let root = tempfile::tempdir().unwrap();
        let server =
            Fixture::new(move |_, _| Reply::ok(wire(case, false, Usage::Absent, false, false)));
        let mut p = case.provider(&server.url, root.path().into());
        p.complete(&user(), &[], &context()).unwrap();
        assert_eq!(
            p.take_metadata(),
            Some(ResponseMetadata::default()),
            "{case:?}"
        );
    }
}
#[test]
fn matrix_tool_cycle_has_independent_file_oracle_and_explicit_error_result() {
    for case in cases() {
        for fail in [false, true] {
            eprintln!("{case:?} error={fail}");
            let root = tempfile::tempdir().unwrap();
            let server = Fixture::new(move |r, i| {
                case.verify(r);
                if i > 0 {
                    verify_tool_result(case, &r.body, fail);
                    let body = r.body.to_string();
                    assert!(body.contains(if fail {
                        "TOOL_DENIED"
                    } else {
                        "ORACLE_WRITTEN"
                    }));
                    assert!(!body.contains("credential_tag"));
                }
                Reply::ok(wire(case, i == 0, Usage::Known, false, false))
            });
            let mut p = case.provider(&server.url, root.path().join("state"));
            let mut tools = oracle(root.path(), fail);
            let turn = run_turn(
                &mut p,
                &mut tools,
                "fixture",
                limits(),
                Cancellation::default(),
            )
            .unwrap();
            assert_eq!(turn.output, "CONFORMANCE_OK");
            assert_eq!(turn.stats.provider_calls, 2);
            assert_eq!(turn.stats.tool_calls, 1);
            assert_eq!(tools.calls, 1);
            assert_eq!(tools.path.exists(), !fail);
            if !fail {
                assert_eq!(std::fs::read(&tools.path).unwrap(), b"42\n");
            }
            let usage = turn.stats.usage;
            assert_eq!(usage.input_tokens.unwrap().sum, 6);
            assert_eq!(usage.output_tokens.unwrap().sum, 4);
            assert_eq!(usage.total_tokens.unwrap().sum, 10);
            assert_eq!(server.requests().len(), 2);
        }
    }
}
fn refuses_tool_response(usage: Usage, incomplete: bool, malformed: bool) {
    for case in cases() {
        eprintln!("{case:?}");
        let root = tempfile::tempdir().unwrap();
        let server =
            Fixture::new(move |_, _| Reply::ok(wire(case, true, usage, incomplete, malformed)));
        let mut p = case.provider(&server.url, root.path().join("state"));
        let mut tools = oracle(root.path(), false);
        assert!(
            run_turn(
                &mut p,
                &mut tools,
                "fixture",
                limits(),
                Cancellation::default()
            )
            .is_err()
        );
        assert_eq!(tools.calls, 0);
        assert!(!tools.path.exists());
        assert!(p.take_metadata().is_none());
        assert!(p.take_state().is_none());
        assert_eq!(server.requests().len(), 1);
    }
}
#[test]
fn matrix_invalid_usage_prevents_tool_effects() {
    refuses_tool_response(Usage::Invalid, false, false);
}
#[test]
fn matrix_incomplete_response_prevents_tool_effects() {
    refuses_tool_response(Usage::Known, true, false);
}
#[test]
fn matrix_malformed_arguments_prevent_tool_effects() {
    refuses_tool_response(Usage::Known, false, true);
}
#[test]
fn matrix_preflight_schema_cancel_and_expired_deadline_send_nothing() {
    for case in cases() {
        let root = tempfile::tempdir().unwrap();
        let server = Fixture::new(|_, _| panic!("preflight sent HTTP"));
        let mut p = case.provider(&server.url, root.path().into());
        let mut bad = oracle(root.path(), false).definitions();
        bad[0].name = "bad\nname".into();
        assert!(p.complete(&user(), &bad, &context()).is_err());
        let signal = Cancellation::default();
        signal.cancel();
        assert_eq!(
            p.complete(
                &user(),
                &[],
                &TurnContext::new(Duration::from_secs(10), signal).unwrap()
            )
            .unwrap_err(),
            AgentError::Cancelled
        );
        let expired = TurnContext::new(Duration::from_nanos(1), Cancellation::default()).unwrap();
        while expired.remaining().is_ok() {
            std::thread::yield_now();
        }
        assert_eq!(
            p.complete(&user(), &[], &expired).unwrap_err(),
            AgentError::Timeout
        );
        assert!(server.requests().is_empty());
    }
}
#[test]
fn matrix_http_errors_are_typed_redacted_and_never_retried() {
    for case in cases() {
        for (status, kind) in [
            (400, FailureKind::InvalidRequest),
            (401, FailureKind::Authentication),
            (403, FailureKind::Permission),
            (404, FailureKind::NotFound),
            (429, FailureKind::RateLimited),
            (503, FailureKind::Unavailable),
        ] {
            let root = tempfile::tempdir().unwrap();
            let server = Fixture::new(move |_, _| Reply {
                status,
                headers: "Retry-After: 7\r\n".into(),
                body: "PRIVATE_ERROR conformance-fake-key".into(),
                extra: 0,
            });
            let mut p = case.provider(&server.url, root.path().into());
            let error = p.complete(&user(), &[], &context()).unwrap_err();
            let failure = error.provider_failure().unwrap();
            assert_eq!(failure.kind, kind, "{case:?}");
            assert_eq!(failure.http_status, Some(status));
            assert_eq!(failure.retry_after_seconds, Some(7));
            assert!(!format!("{error:?}").contains("PRIVATE_ERROR"));
            assert!(!error.to_string().contains("conformance-fake-key"));
            assert!(p.take_metadata().is_none());
            assert_eq!(server.requests().len(), 1);
        }
    }
}
#[test]
fn matrix_redirects_do_not_forward_credentials_or_generate_fallback() {
    for case in cases() {
        let root = tempfile::tempdir().unwrap();
        let target = Fixture::new(|_, _| panic!("followed redirect"));
        let location = format!("Location: {}/stolen\r\n", target.url);
        let server = Fixture::new(move |_, _| Reply {
            status: 307,
            headers: location.clone(),
            body: String::new(),
            extra: 0,
        });
        let mut p = case.provider(&server.url, root.path().into());
        assert!(p.complete(&user(), &[], &context()).is_err());
        assert_eq!(server.requests().len(), 1);
        assert!(target.requests().is_empty());
    }
}
#[test]
fn matrix_transport_truncation_has_no_metadata_or_tool_effect() {
    for case in cases() {
        let root = tempfile::tempdir().unwrap();
        let server = Fixture::new(move |_, _| {
            let mut r = Reply::ok(wire(case, true, Usage::Known, false, false));
            r.body.truncate(r.body.len() / 2);
            r.extra = 100;
            r
        });
        let mut p = case.provider(&server.url, root.path().join("state"));
        let mut tools = oracle(root.path(), false);
        assert!(
            run_turn(
                &mut p,
                &mut tools,
                "fixture",
                limits(),
                Cancellation::default()
            )
            .is_err(),
            "{case:?}"
        );
        assert_eq!(tools.calls, 0);
        assert!(!tools.path.exists());
        assert!(p.take_metadata().is_none());
        assert!(p.take_state().is_none());
        assert_eq!(server.requests().len(), 1);
    }
}
#[test]
fn matrix_cancel_during_headers_and_partial_body_prevents_effects() {
    for case in cases() {
        for body in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let mut p = case.provider(&url, root.path().join("state"));
            let signal = Cancellation::default();
            let cancel = signal.clone();
            let (release, hold) = mpsc::channel();
            let server = std::thread::spawn(move || {
                let mut s = accept_bounded(listener);
                read_request(&mut s);
                if body {
                    write!(s, "HTTP/1.1 200 OK\r\nContent-Length: 900000\r\n\r\n{{").unwrap();
                    s.flush().unwrap();
                }
                cancel.cancel();
                hold.recv_timeout(Duration::from_secs(10)).unwrap();
            });
            let mut tools = oracle(root.path(), false);
            let result = run_turn(&mut p, &mut tools, "fixture", limits(), signal);
            release.send(()).unwrap();
            server.join().unwrap();
            assert_eq!(
                result.unwrap_err(),
                AgentError::Cancelled,
                "{case:?} body={body}"
            );
            assert_eq!(tools.calls, 0);
            assert!(!tools.path.exists());
            assert!(p.take_state().is_none());
            assert!(p.take_metadata().is_none());
        }
    }
}

#[test]
fn matrix_png_roundtrip_and_known_model_capabilities_reject_before_http() {
    for case in cases() {
        eprintln!("{case:?}");
        let root = tempfile::tempdir().unwrap();
        let image = ToolImage { width:2, height:2, png_base64:"iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEklEQVR4nGP4z8DAAMIM/4EAAB/uBfsL2WiLAAAAAElFTkSuQmCC".into() };
        image.png().unwrap();
        let encoded = image.png_base64.clone();
        let server = Fixture::new(move |r, _| {
            assert!(r.body.to_string().contains(&encoded));
            Reply::ok(wire(case, false, Usage::Known, false, false))
        });
        let mut p = case.provider(&server.url, root.path().into());
        let mut messages = user();
        messages.extend([
            Message::Assistant {
                content: String::new(),
                tool_calls: vec![ToolCall {
                    id: "screen".into(),
                    name: "screen".into(),
                    arguments: json!({}),
                }],
            },
            Message::Tool {
                tool_call_id: "screen".into(),
                content: "pixels".into(),
                is_error: false,
            },
            Message::ToolImage {
                tool_call_id: "screen".into(),
                image,
            },
        ]);
        p.complete(&messages, &[], &context()).unwrap();
        let model = crate::catalog::Model {
            id: case.model().into(),
            tools: Some(false),
            images: Some(false),
            ..Default::default()
        };
        let mut p = match p {
            Driver::Api(p) => Driver::Api(p.with_model(model).unwrap()),
            Driver::Codex(p) => Driver::Codex(p.with_model(model).unwrap()),
        };
        assert!(p.complete(&messages, &[], &context()).is_err());
        assert!(
            p.complete(
                &user(),
                &oracle(root.path(), false).definitions(),
                &context()
            )
            .is_err()
        );
        assert_eq!(server.requests().len(), 1);
    }
}

#[test]
fn matrix_deadline_during_http_wait_does_not_publish_partial_tools() {
    for case in cases() {
        let root = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let mut p = case.provider(&url, root.path().join("state"));
        let (release, hold) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let mut s = accept_bounded(listener);
            read_request(&mut s);
            write!(s, "HTTP/1.1 200 OK\r\nContent-Length: 900000\r\n\r\n{{").unwrap();
            s.flush().unwrap();
            hold.recv_timeout(Duration::from_secs(10)).unwrap();
        });
        let mut tools = oracle(root.path(), false);
        // The server deliberately holds an unfinished body. Check the typed outcome,
        // never a narrow elapsed-time window or a race against response publication.
        let mut budget = limits();
        budget.timeout = Duration::from_millis(500);
        let result = run_turn(
            &mut p,
            &mut tools,
            "fixture",
            budget,
            Cancellation::default(),
        );
        release.send(()).unwrap();
        server.join().unwrap();
        assert_eq!(result.unwrap_err(), AgentError::Timeout, "{case:?}");
        assert_eq!(tools.calls, 0);
        assert!(!tools.path.exists());
        assert!(p.take_metadata().is_none());
        assert!(p.take_state().is_none());
    }
}
