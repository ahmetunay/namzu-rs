use namzu_core::{AgentError, Cancellation, TurnContext};
use namzu_mcp::{
    Incoming, Phase,
    http::{Client, Error, Spec},
    types::Implementation,
};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone)]
struct Request {
    method: String,
    headers: String,
    body: Value,
}
struct Reply {
    status: u16,
    headers: String,
    body: Vec<u8>,
    fragment: bool,
    stall: bool,
}
fn reply(status: u16, headers: &str, body: impl Into<Vec<u8>>) -> Reply {
    Reply {
        status,
        headers: headers.into(),
        body: body.into(),
        fragment: false,
        stall: false,
    }
}
fn json_reply(value: Value) -> Reply {
    reply(
        200,
        "Content-Type: application/json\r\n",
        serde_json::to_vec(&value).unwrap(),
    )
}
fn initialized(req: &Request, headers: &str) -> Reply {
    let mut r = json_reply(
        json!({"jsonrpc":"2.0","id":req.body["id"],"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"loopback","version":"1"}}}),
    );
    r.headers.push_str(headers);
    r
}
fn read_request(socket: &mut impl Read) -> Request {
    let mut bytes = vec![];
    let mut byte = [0];
    while !bytes.ends_with(b"\r\n\r\n") {
        socket.read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
        assert!(bytes.len() < 16384);
    }
    let headers = String::from_utf8(bytes).unwrap();
    let size = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length: ")
                .map(|v| v.parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    let mut body = vec![0; size];
    socket.read_exact(&mut body).unwrap();
    Request {
        method: headers.split_whitespace().next().unwrap().into(),
        headers: headers.to_ascii_lowercase(),
        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
    }
}
type Step = Box<dyn FnOnce(&Request) -> Reply + Send>;
struct Server {
    url: String,
    requests: Arc<Mutex<Vec<Request>>>,
    handle: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new(steps: Vec<Step>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(vec![]));
        let log = requests.clone();
        let handle = thread::spawn(move || {
            for step in steps {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut socket = loop {
                    match listener.accept() {
                        Ok((s, _)) => break s,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline, "missing request");
                            thread::sleep(Duration::from_millis(1));
                        }
                        Err(e) => panic!("{e}"),
                    }
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let request = read_request(&mut socket);
                log.lock().unwrap().push(request.clone());
                let r = step(&request);
                let head = format!(
                    "HTTP/1.1 {} fixture\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    r.status,
                    r.headers,
                    if r.stall { 10000 } else { r.body.len() }
                );
                if socket.write_all(head.as_bytes()).is_err() {
                    continue;
                }
                if r.stall {
                    let mut b = [0];
                    let _ = socket.read(&mut b);
                } else if r.fragment {
                    for b in r.body {
                        if socket.write_all(&[b]).is_err() {
                            break;
                        }
                    }
                } else {
                    let _ = socket.write_all(&r.body);
                }
            }
        });
        Self {
            url,
            requests,
            handle: Some(handle),
        }
    }
    fn connect(&self) -> Client {
        Client::connect(
            Spec::new(&self.url),
            Implementation::new("test", "1"),
            &context(),
        )
        .unwrap()
    }
    fn finish(mut self) -> Vec<Request> {
        self.handle.take().unwrap().join().unwrap();
        self.requests.lock().unwrap().clone()
    }
}
fn context() -> TurnContext {
    TurnContext::new(Duration::from_secs(3), Cancellation::default()).unwrap()
}
fn handshake() -> Vec<Step> {
    vec![
        Box::new(|r| {
            assert_eq!(r.body["method"], "initialize");
            assert!(!r.headers.contains("mcp-session-id:"));
            initialized(r, "MCP-Session-Id: opaque-fixture\r\n")
        }),
        Box::new(|r| {
            assert_eq!(r.body["method"], "notifications/initialized");
            assert!(r.headers.contains("mcp-session-id: opaque-fixture"));
            assert!(r.headers.contains("mcp-protocol-version: 2025-06-18"));
            reply(202, "", vec![])
        }),
    ]
}
fn response(req: &Request) -> Value {
    json!({"jsonrpc":"2.0","id":req.body["id"],"result":{}})
}
fn sse(body: String) -> Reply {
    reply(
        200,
        "Content-Type: text/event-stream; charset=utf-8\r\n",
        body.into_bytes(),
    )
}

#[test]
fn json_negotiated_headers_auth_and_explicit_delete() {
    let mut steps = handshake();
    steps.push(Box::new(|r| {
        assert!(r.headers.contains("authorization: bearer fixture-token"));
        assert!(
            r.headers
                .contains("accept: application/json, text/event-stream")
        );
        json_reply(response(r))
    }));
    steps.push(Box::new(|r| {
        assert_eq!(r.method, "DELETE");
        reply(204, "", vec![])
    }));
    let server = Server::new(steps);
    let mut spec = Spec::new(&server.url);
    spec.bearer = Some("fixture-token".into());
    let mut client = Client::connect(spec, Implementation::new("test", "1"), &context()).unwrap();
    assert!(matches!(
        client.call("ping", None, &context()).unwrap(),
        Incoming::Reply { result: Ok(_), .. }
    ));
    client.close(&context()).unwrap();
    assert_eq!(client.phase(), Phase::Closed);
    assert!(client.abandoned().is_empty());
    assert_eq!(server.finish().len(), 4);
}
#[test]
fn fragmented_sse_priming_progress_and_server_requests() {
    let mut steps = handshake();
    steps.push(Box::new(|r| {let mut v=sse(format!("\u{feff}: keepalive\r\nid: prime\r\ndata:\r\n\r\ndata: {{\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{{\"text\":\"İstanbul 🦀\"}}}}\r\n\r\ndata: {{\"jsonrpc\":\"2.0\",\"id\":\"peer\",\"method\":\"ping\"}}\r\n\r\ndata: {}\r\n\r\n",response(r)));v.fragment=true;v}));
    steps.push(Box::new(|r| {
        assert_eq!(r.body["id"], "peer");
        assert_eq!(r.body["result"], json!({}));
        reply(202, "", vec![])
    }));
    let server = Server::new(steps);
    let mut client = server.connect();
    client.call("ping", None, &context()).unwrap();
    assert!(
        matches!(client.pop_event(),Some(Incoming::Notification{params:Some(p),..}) if p["text"]=="İstanbul 🦀")
    );
    server.finish();
}
#[test]
fn initialize_can_itself_be_sse_and_sessionless() {
    let server = Server::new(vec![
        Box::new(|r| {
            let init = initialized(r, "");
            sse(format!(
                "data: {}\n\n",
                String::from_utf8(init.body).unwrap()
            ))
        }),
        Box::new(|r| {
            assert!(!r.headers.contains("mcp-session-id:"));
            reply(202, "", vec![])
        }),
    ]);
    let mut client = server.connect();
    client.close(&context()).unwrap();
    server.finish();
}
#[test]
fn optional_get_preserves_multiple_events_on_one_stream() {
    let mut steps = handshake();
    steps.push(Box::new(|r|{assert_eq!(r.method,"GET");assert!(r.headers.contains("accept: text/event-stream\r\n"));sse("data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/prompts/list_changed\"}\n\n".into())}));
    steps.push(Box::new(|r| {
        assert_eq!(r.method, "DELETE");
        reply(405, "", vec![])
    }));
    let server = Server::new(steps);
    let mut client = server.connect();
    assert!(
        matches!(client.listen(&context()).unwrap(),Some(Incoming::Notification{method,..}) if method=="notifications/tools/list_changed")
    );
    assert!(
        matches!(client.listen(&context()).unwrap(),Some(Incoming::Notification{method,..}) if method=="notifications/prompts/list_changed")
    );
    client.close(&context()).unwrap();
    assert_eq!(server.finish().len(), 4);
}
#[test]
fn get_405_does_not_close_session() {
    let mut steps = handshake();
    steps.push(Box::new(|_| reply(405, "", vec![])));
    let server = Server::new(steps);
    let mut client = server.connect();
    assert_eq!(client.listen(&context()).unwrap(), None);
    assert_eq!(client.phase(), Phase::Ready);
    server.finish();
}
#[test]
fn unsolicited_get_response_fences_generation() {
    let mut steps = handshake();
    steps.push(Box::new(|_| {
        sse("data: {\"jsonrpc\":\"2.0\",\"id\":\"mcp-1\",\"result\":{}}\n\n".into())
    }));
    let server = Server::new(steps);
    let mut client = server.connect();
    assert_eq!(client.listen(&context()), Err(Error::Stream));
    assert_eq!(client.phase(), Phase::Closed);
    server.finish();
}
fn failure_case(make: impl FnOnce(&Request) -> Reply + Send + 'static, expected: Error) {
    let mut steps = handshake();
    steps.push(Box::new(make));
    let server = Server::new(steps);
    let mut client = server.connect();
    assert_eq!(
        client.call(
            "tools/call",
            Some(
                json!({"name":"effect","arguments":{}})
                    .as_object()
                    .unwrap()
                    .clone()
            ),
            &context()
        ),
        Err(expected)
    );
    assert_eq!(client.phase(), Phase::Closed);
    assert_eq!(client.abandoned().len(), 1);
    assert_eq!(client.abandoned()[0].method, "tools/call");
    assert!(client.call("ping", None, &context()).is_err());
    assert_eq!(server.finish().len(), 3);
}
#[test]
fn session_404_never_reinitializes_or_replays() {
    failure_case(
        |_| reply(404, "", b"secret".to_vec()),
        Error::SessionExpired,
    );
}
#[test]
fn redirects_never_follow_or_replay() {
    failure_case(
        |_| reply(307, "Location: http://127.0.0.1:1/steal\r\n", vec![]),
        Error::Status(307),
    );
}
#[test]
fn server_failure_body_not_in_error() {
    failure_case(
        |_| reply(500, "", b"secret credentials".to_vec()),
        Error::Status(500),
    );
}
#[test]
fn wrong_id_retains_uncertainty() {
    failure_case(
        |_| json_reply(json!({"jsonrpc":"2.0","id":"wrong","result":{}})),
        Error::Protocol(namzu_mcp::Error::ResponseId),
    );
}
#[test]
fn truncated_sse_never_accepts_partial_result() {
    failure_case(|r| sse(format!("data: {}\n", response(r))), Error::Stream);
}
#[test]
fn sse_disconnect_never_reposts() {
    failure_case(
        |_| sse("id: cursor\nretry: 0\ndata:\n\n".into()),
        Error::Stream,
    );
}
#[test]
fn session_header_cannot_be_replaced() {
    failure_case(
        |r| {
            let mut v = json_reply(response(r));
            v.headers.push_str("MCP-Session-Id: stolen\r\n");
            v
        },
        Error::Headers,
    );
}
#[test]
fn duplicate_session_headers_rejected() {
    failure_case(
        |r| {
            let mut v = json_reply(response(r));
            v.headers
                .push_str("MCP-Session-Id: opaque-fixture\r\nMCP-Session-Id: opaque-fixture\r\n");
            v
        },
        Error::Headers,
    );
}
#[test]
fn unexpected_media_type_rejected() {
    failure_case(
        |r| {
            reply(
                200,
                "Content-Type: text/html\r\n",
                serde_json::to_vec(&response(r)).unwrap(),
            )
        },
        Error::Headers,
    );
}
#[test]
fn bounded_sse_line() {
    failure_case(
        |_| sse(format!("data: {}\n\n", "x".repeat(1024 * 1024 + 1))),
        Error::Limit,
    );
}
#[test]
fn bounded_json_body() {
    failure_case(
        |_| {
            reply(
                200,
                "Content-Type: application/json\r\n",
                vec![b' '; 1024 * 1024 + 1],
            )
        },
        Error::Limit,
    );
}
#[test]
fn notification_backlog_is_bounded() {
    failure_case(
        |r| {
            sse(format!(
                "{}data: {}\n\n",
                "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\n".repeat(65),
                response(r)
            ))
        },
        Error::Limit,
    );
}
#[test]
fn cancellation_during_body_closes_future_and_retains_effect() {
    let cancellation = Cancellation::default();
    let trigger = cancellation.clone();
    let mut steps = handshake();
    steps.push(Box::new(move |_| {
        trigger.cancel();
        let mut v = sse(String::new());
        v.stall = true;
        v
    }));
    let server = Server::new(steps);
    let mut client = server.connect();
    let ctx = TurnContext::new(Duration::from_secs(30), cancellation).unwrap();
    assert_eq!(
        client.call("ping", None, &ctx),
        Err(Error::Context(AgentError::Cancelled))
    );
    assert_eq!(client.abandoned().len(), 1);
    server.finish();
}
#[test]
fn deadline_during_body_closes_generation() {
    let mut steps = handshake();
    steps.push(Box::new(|_| {
        let mut v = sse(String::new());
        v.stall = true;
        v
    }));
    let server = Server::new(steps);
    let mut client = server.connect();
    let ctx = TurnContext::new(Duration::from_millis(100), Cancellation::default()).unwrap();
    assert_eq!(
        client.call("ping", None, &ctx),
        Err(Error::Context(AgentError::Timeout))
    );
    assert_eq!(client.abandoned().len(), 1);
    server.finish();
}
#[test]
fn invalid_endpoint_and_token_rejected_before_network() {
    for endpoint in [
        "http://example.com/mcp",
        "file:///tmp/a",
        "https://user:pass@example.com/mcp",
        "https://example.com/mcp#fragment",
    ] {
        assert!(matches!(
            Client::connect(
                Spec::new(endpoint),
                Implementation::new("test", "1"),
                &context()
            ),
            Err(Error::Settings)
        ));
    }
    let mut spec = Spec::new("http://127.0.0.1:1/mcp");
    spec.bearer = Some("a\r\nInjected: b".into());
    assert!(matches!(
        Client::connect(spec, Implementation::new("test", "1"), &context()),
        Err(Error::Settings)
    ));
}
#[test]
fn initialization_invalid_session_and_nonempty_ack_rejected() {
    for invalid_id in [true, false] {
        let mut steps: Vec<Step> = vec![Box::new(move |r| {
            initialized(
                r,
                if invalid_id {
                    "MCP-Session-Id: invalid id\r\n"
                } else {
                    ""
                },
            )
        })];
        if !invalid_id {
            steps.push(Box::new(|_| reply(202, "", b"unexpected".to_vec())));
        }
        let server = Server::new(steps);
        assert!(
            Client::connect(
                Spec::new(&server.url),
                Implementation::new("test", "1"),
                &context()
            )
            .is_err()
        );
        server.finish();
    }
}

#[test]
fn remote_rpc_and_tool_errors_leave_transport_usable() {
    let mut steps = handshake();
    steps.push(Box::new(|r| {
        json_reply(
            json!({"jsonrpc":"2.0","id":r.body["id"],"error":{"code":-32602,"message":"fixture"}}),
        )
    }));
    steps.push(Box::new(|r| {
        json_reply(
            json!({"jsonrpc":"2.0","id":r.body["id"],"result":{"content":[],"isError":true}}),
        )
    }));
    let server = Server::new(steps);
    let mut client = server.connect();
    assert!(matches!(
        client.call("ping", None, &context()).unwrap(),
        Incoming::Reply { result: Err(_), .. }
    ));
    assert!(
        matches!(client.call("tools/call",None,&context()).unwrap(),Incoming::Reply{result:Ok(r),..} if r["isError"]==true)
    );
    assert_eq!(client.phase(), Phase::Ready);
    assert!(client.abandoned().is_empty());
    server.finish();
}
#[test]
fn cancelled_before_admission_sends_nothing_and_does_not_break_session() {
    let server = Server::new(handshake());
    let mut client = server.connect();
    let ctx = context();
    ctx.cancellation().cancel();
    assert!(client.call("ping", None, &ctx).is_err());
    assert_eq!(client.phase(), Phase::Ready);
    assert!(client.abandoned().is_empty());
    assert_eq!(server.finish().len(), 2);
}
#[test]
fn tls_fixture_child() {
    let Ok(expected) = std::env::var("NAMZU_MCP_TLS_FIXTURE") else {
        return;
    };
    let result = Client::connect(
        Spec::new("https://mcp.fixture.invalid/mcp"),
        Implementation::new("fixture", "1"),
        &context(),
    );
    if expected == "trusted" {
        let mut client = result.unwrap();
        client.call("ping", None, &context()).unwrap();
        client.close(&context()).unwrap();
    } else {
        assert!(matches!(result, Err(Error::Network)));
    }
}
fn proxy_tls_fixture(trusted: bool) {
    use std::process::{Command, Stdio};
    let dir = tempfile::tempdir().unwrap();
    let cert = dir.path().join("cert.pem");
    let key = dir.path().join("key.pem");
    let status = Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "1",
            "-subj",
            "/CN=mcp.fixture.invalid",
            "-addext",
            "subjectAltName=DNS:mcp.fixture.invalid",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
            "-keyout",
        ])
        .arg(&key)
        .arg("-out")
        .arg(&cert)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    let identity = native_tls::Identity::from_pkcs8(
        &std::fs::read(&cert).unwrap(),
        &std::fs::read(&key).unwrap(),
    )
    .unwrap();
    let acceptor = native_tls::TlsAcceptor::new(identity).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let proxy = format!("http://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut socket = loop {
            match listener.accept() {
                Ok((s, _)) => break s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline);
                    thread::sleep(Duration::from_millis(1));
                }
                Err(e) => panic!("{e}"),
            }
        };
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let request = read_request(&mut socket);
        assert_eq!(request.method, "CONNECT");
        assert!(
            request
                .headers
                .starts_with("connect mcp.fixture.invalid:443 ")
        );
        socket
            .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .unwrap();
        let tls = acceptor.accept(socket);
        if !trusted {
            assert!(tls.is_err());
            return;
        }
        let mut tls = tls.unwrap();
        for i in 0..4 {
            let r = read_request(&mut tls);
            let response = match i {
                0 => initialized(&r, "MCP-Session-Id: tls-session\r\n"),
                1 => reply(202, "", vec![]),
                2 => json_reply(response(&r)),
                _ => {
                    assert_eq!(r.method, "DELETE");
                    reply(204, "", vec![])
                }
            };
            tls.write_all(
                format!(
                    "HTTP/1.1 {} fixture\r\n{}Content-Length: {}\r\n\r\n",
                    response.status,
                    response.headers,
                    response.body.len()
                )
                .as_bytes(),
            )
            .unwrap();
            tls.write_all(&response.body).unwrap();
            tls.flush().unwrap();
        }
    });
    let mut child = Command::new(std::env::current_exe().unwrap());
    child
        .args(["--exact", "tls_fixture_child", "--nocapture"])
        .env(
            "NAMZU_MCP_TLS_FIXTURE",
            if trusted { "trusted" } else { "untrusted" },
        );
    for name in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
    ] {
        child.env_remove(name);
    }
    child.env("HTTPS_PROXY", proxy).env("NO_PROXY", "");
    if trusted {
        child.env("SSL_CERT_FILE", &cert);
    } // child-local CA; no process-global env mutation
    let output = child.output().unwrap();
    server.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}
#[test]
fn managed_ca_and_https_connect_proxy_are_used() {
    proxy_tls_fixture(true);
}
#[test]
fn tls_verification_rejects_untrusted_proxy_certificate() {
    proxy_tls_fixture(false);
}
fn manager_registry(root: &std::path::Path, url: &str) -> namzu_mcp::config::Registry {
    let r = namzu_mcp::config::Registry::open(root).unwrap();
    r.replace(namzu_mcp::config::Document::parse(json!({"version":1,"default_profile":"default","profiles":{"default":{"servers":{"fixture":{"transport":{"type":"http","url":url},"policy":{"require_approval":false}}}}}}).to_string().as_bytes()).unwrap()).unwrap();
    r
}
fn manager_ceiling() -> namzu_mcp::config::Policy {
    namzu_mcp::config::Policy {
        require_approval: false,
        ..Default::default()
    }
}
#[test]
fn manager_session_expiry_reconnect_is_fresh_and_does_not_replay_rpc() {
    use namzu_mcp::manager::Manager;
    let s = Server::new(vec![
        Box::new(|r| initialized(r, "MCP-Session-Id: old\r\n")),
        Box::new(|_| reply(202, "", vec![])),
        Box::new(|r| {
            assert_eq!(r.body["method"], "ping");
            reply(404, "", vec![])
        }),
        Box::new(|r| {
            assert_eq!(r.method, "DELETE");
            reply(404, "", vec![])
        }),
        Box::new(|r| {
            assert_eq!(r.body["method"], "initialize");
            assert!(!r.headers.contains("mcp-session-id"));
            initialized(r, "MCP-Session-Id: fresh\r\n")
        }),
        Box::new(|_| reply(202, "", vec![])),
        Box::new(|r| {
            assert!(r.headers.contains("mcp-session-id: fresh"));
            json_reply(json!({"jsonrpc":"2.0","id":r.body["id"],"result":{}}))
        }),
        Box::new(|r| {
            assert_eq!(r.method, "DELETE");
            reply(200, "", vec![])
        }),
    ]);
    let root = tempfile::tempdir().unwrap();
    let r = manager_registry(root.path(), &s.url);
    let c = context();
    let mut m = Manager::new(&r, None, None, manager_ceiling(), &c).unwrap();
    let p = m.prepare_connect("fixture").unwrap().permit().unwrap();
    m.execute(p, &c).unwrap();
    let p = m
        .prepare_call("fixture", "ping", None)
        .unwrap()
        .permit()
        .unwrap();
    assert!(m.execute(p, &c).is_err());
    let p = m.prepare_reconnect("fixture").unwrap().permit().unwrap();
    m.execute(p, &c).unwrap();
    let p = m
        .prepare_call("fixture", "ping", None)
        .unwrap()
        .permit()
        .unwrap();
    m.execute(p, &c).unwrap();
    m.close(&c).unwrap();
    let requests = s.finish();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.body["method"] == "ping")
            .count(),
        2
    );
    assert!(r.recovery().unwrap().list().unwrap().is_empty());
}
fn resource_initialized(r: &Request) -> Reply {
    json_reply(
        json!({"jsonrpc":"2.0","id":r.body["id"],"result":{"protocolVersion":"2025-11-25","capabilities":{"resources":{"subscribe":true}},"serverInfo":{"name":"independent-http-resource-peer","version":"1"}}}),
    )
}
#[test]
fn manager_http_subscription_sse_and_unsubscribe_use_one_generation() {
    use namzu_mcp::manager::Manager;
    let s = Server::new(vec![
        Box::new(resource_initialized),
        Box::new(|_| reply(202, "", vec![])),
        Box::new(|r| {
            assert_eq!(r.body["method"], "resources/subscribe");
            json_reply(json!({"jsonrpc":"2.0","id":r.body["id"],"result":{}}))
        }),
        Box::new(|r| {
            assert_eq!(r.method, "GET");
            assert!(!r.headers.contains("last-event-id"));
            reply(200,"Content-Type: text/event-stream\r\n",b"id: event-1\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/resources/updated\",\"params\":{\"uri\":\"mem://data\"}}\n\n".to_vec())
        }),
        Box::new(|r| {
            assert_eq!(r.body["method"], "resources/unsubscribe");
            json_reply(json!({"jsonrpc":"2.0","id":r.body["id"],"result":{}}))
        }),
    ]);
    let root = tempfile::tempdir().unwrap();
    let r = manager_registry(root.path(), &s.url);
    let c = context();
    let mut m = Manager::new(&r, None, None, manager_ceiling(), &c).unwrap();
    let p = m.prepare_connect("fixture").unwrap().permit().unwrap();
    m.execute(p, &c).unwrap();
    let params = Some(json!({"uri":"mem://data"}).as_object().unwrap().clone());
    let p = m
        .prepare_call("fixture", "resources/subscribe", params.clone())
        .unwrap()
        .permit()
        .unwrap();
    m.execute(p, &c).unwrap();
    let p = m
        .prepare_notifications("fixture")
        .unwrap()
        .permit()
        .unwrap();
    m.execute(p, &c).unwrap();
    assert_eq!(
        m.take_resource_updates("fixture").unwrap(),
        vec!["mem://data"]
    );
    assert_eq!(
        m.pop_notification().unwrap().generation,
        m.statuses()[0].generation
    );
    let p = m
        .prepare_call("fixture", "resources/unsubscribe", params)
        .unwrap()
        .permit()
        .unwrap();
    m.execute(p, &c).unwrap();
    assert!(m.subscriptions("fixture").unwrap().is_empty());
    m.close(&c).unwrap();
    assert_eq!(s.finish().len(), 5);
}
#[test]
fn http_tool_401_is_durable_uncertain_and_cannot_reconnect_automatically() {
    use namzu_mcp::{config::Error as ManagerError, manager::Manager};
    let mut steps = handshake();
    steps.push(Box::new(|r| {
        assert_eq!(r.body["method"], "tools/call");
        reply(401, "", b"DO_NOT_PRINT_ERROR_SECRET".to_vec())
    }));
    steps.push(Box::new(|r| {
        assert_eq!(r.method, "DELETE");
        reply(200, "", vec![])
    }));
    let s = Server::new(steps);
    let root = tempfile::tempdir().unwrap();
    let r = manager_registry(root.path(), &s.url);
    let c = context();
    let mut m = Manager::new(&r, None, None, manager_ceiling(), &c).unwrap();
    let p = m.prepare_connect("fixture").unwrap().permit().unwrap();
    m.execute(p, &c).unwrap();
    let p = m
        .prepare_call(
            "fixture",
            "tools/call",
            Some(
                json!({"name":"effect","arguments":{}})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .unwrap()
        .permit()
        .unwrap();
    assert!(m.execute(p, &c).is_err());
    let p = m.prepare_reconnect("fixture").unwrap().permit().unwrap();
    assert!(matches!(m.execute(p, &c), Err(ManagerError::Uncertain)));
    assert!(r.recovery().unwrap().list().unwrap()[0].uncertain);
    m.close(&c).unwrap();
    assert_eq!(s.finish().len(), 4);
}
#[test]
fn sse_eof_discards_subscriptions_and_requires_explicit_fresh_stream() {
    use namzu_mcp::manager::Manager;
    fn notification() -> Reply {
        reply(200,"Content-Type: text/event-stream\r\n",b"data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/resources/updated\",\"params\":{\"uri\":\"mem://data\"}}\n\n".to_vec())
    }
    let s = Server::new(vec![
        Box::new(resource_initialized),
        Box::new(|_| reply(202, "", vec![])),
        Box::new(|r| json_reply(response(r))),
        Box::new(|r| {
            assert_eq!(r.method, "GET");
            notification()
        }),
        Box::new(|r| {
            assert_eq!(r.body["method"], "initialize");
            resource_initialized(r)
        }),
        Box::new(|_| reply(202, "", vec![])),
        Box::new(|r| {
            assert_eq!(r.body["method"], "resources/subscribe");
            json_reply(response(r))
        }),
        Box::new(|r| {
            assert_eq!(r.method, "GET");
            notification()
        }),
    ]);
    let root = tempfile::tempdir().unwrap();
    let r = manager_registry(root.path(), &s.url);
    let c = context();
    let mut m = Manager::new(&r, None, None, manager_ceiling(), &c).unwrap();
    let p = m.prepare_connect("fixture").unwrap().permit().unwrap();
    m.execute(p, &c).unwrap();
    let params = Some(json!({"uri":"mem://data"}).as_object().unwrap().clone());
    let p = m
        .prepare_call("fixture", "resources/subscribe", params.clone())
        .unwrap()
        .permit()
        .unwrap();
    m.execute(p, &c).unwrap();
    let p = m
        .prepare_notifications("fixture")
        .unwrap()
        .permit()
        .unwrap();
    m.execute(p, &c).unwrap();
    assert_eq!(
        m.take_resource_updates("fixture").unwrap(),
        vec!["mem://data"]
    );
    let p = m
        .prepare_notifications("fixture")
        .unwrap()
        .permit()
        .unwrap();
    assert!(m.execute(p, &c).is_err());
    assert!(m.subscriptions("fixture").unwrap().is_empty());
    assert_eq!(s.requests.lock().unwrap().len(), 4);
    let p = m.prepare_reconnect("fixture").unwrap().permit().unwrap();
    m.execute(p, &c).unwrap();
    assert!(m.subscriptions("fixture").unwrap().is_empty());
    let p = m
        .prepare_call("fixture", "resources/subscribe", params)
        .unwrap()
        .permit()
        .unwrap();
    m.execute(p, &c).unwrap();
    let p = m
        .prepare_notifications("fixture")
        .unwrap()
        .permit()
        .unwrap();
    m.execute(p, &c).unwrap();
    assert_eq!(
        m.take_resource_updates("fixture").unwrap(),
        vec!["mem://data"]
    );
    m.close(&c).unwrap();
    assert_eq!(
        s.finish()
            .iter()
            .filter(|r| r.body["method"] == "resources/subscribe")
            .count(),
        2
    );
}
