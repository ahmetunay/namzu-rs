use namzu_core::{Cancellation, TurnContext};
use namzu_mcp::{
    config::{Document, Error, Policy, Registry},
    discovery::{Error as DiscoveryError, Kind},
    manager::{Manager, Outcome},
};
use serde_json::{Map, Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::{Duration, Instant},
};
#[test]
fn http_sse_pages_and_get_notifications_invalidate_cached_permissions() {
    fixture(0);
}
#[test]
fn http_change_during_page_never_publishes_or_retries() {
    fixture(1);
}
#[test]
fn http_notification_backpressure_fences_discovery() {
    fixture(2);
}
fn fixture(mode: u8) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let calls = if mode == 0 { 6 } else { 5 };
    let worker = thread::spawn(move || {
        let mut list_calls = 0;
        for index in 0..calls {
            let until = Instant::now() + Duration::from_secs(5);
            let mut socket = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < until);
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut headers = vec![];
            let mut b = [0];
            while !headers.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut b).unwrap();
                headers.push(b[0]);
                assert!(headers.len() < 16384);
            }
            let headers = String::from_utf8(headers).unwrap();
            let size = headers
                .lines()
                .find_map(|line| {
                    line.to_lowercase()
                        .strip_prefix("content-length: ")
                        .map(|v| v.parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            let mut body = vec![0; size];
            socket.read_exact(&mut body).unwrap();
            let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let (status, content, body) = if index == 0 {
                assert_eq!(request["method"], "initialize");
                (200,"application/json",json!({"jsonrpc":"2.0","id":request["id"],"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{"listChanged":true},"prompts":{"listChanged":true}},"serverInfo":{"name":"http-discovery","version":"1"}}}).to_string())
            } else if index == 1 {
                assert_eq!(request["method"], "notifications/initialized");
                (202, "application/json", String::new())
            } else if index == 2 || index == 3 {
                assert_eq!(request["method"], "tools/list");
                list_calls += 1;
                let result = if index == 2 {
                    assert!(request.get("params").is_none());
                    json!({"tools":[{"name":"read","inputSchema":{"type":"object"}}],"nextCursor":"next/+?="})
                } else {
                    assert_eq!(request["params"]["cursor"], "next/+?=");
                    json!({"tools":[{"name":"write","inputSchema":{"type":"object"}}]})
                };
                let notification = if mode == 1 {
                    "notifications/tools/list_changed"
                } else {
                    "notifications/prompts/list_changed"
                };
                let mut body = String::new();
                if index == 3 {
                    if mode == 2 {
                        for _ in 0..65 {
                            body.push_str("data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\n");
                        }
                    } else {
                        body.push_str(&format!(
                            "data: {{\"jsonrpc\":\"2.0\",\"method\":\"{notification}\"}}\n\n"
                        ));
                    }
                }
                body.push_str(&format!(
                    "data: {}\n\n",
                    json!({"jsonrpc":"2.0","id":request["id"],"result":result})
                ));
                (200, "text/event-stream", body)
            } else if mode == 0 && index == 4 {
                assert!(headers.starts_with("GET "));
                (200,"text/event-stream","data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\n".into())
            } else {
                assert!(headers.starts_with("DELETE "));
                (204, "application/json", String::new())
            };
            let head = format!(
                "HTTP/1.1 {status} fixture\r\nContent-Type: {content}\r\nMCP-Session-Id: discovery\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = socket.write_all(head.as_bytes());
            let _ = socket.write_all(body.as_bytes());
        }
        assert_eq!(list_calls, 2);
    });
    let root = tempfile::tempdir().unwrap();
    let registry = Registry::open(&root.path().join("state")).unwrap();
    let doc = json!({"version":1,"default_profile":"default","profiles":{"default":{"servers":{"fixture":{"transport":{"type":"http","url":format!("http://{address}/mcp")},"policy":{"require_approval":false}}}}}});
    registry
        .replace(Document::parse(doc.to_string().as_bytes()).unwrap())
        .unwrap();
    let ctx = TurnContext::new(Duration::from_secs(10), Cancellation::default()).unwrap();
    let mut manager = Manager::new(
        &registry,
        None,
        None,
        Policy {
            require_approval: false,
            ..Policy::default()
        },
        &ctx,
    )
    .unwrap();
    let p = manager
        .prepare_connect("fixture")
        .unwrap()
        .permit()
        .unwrap();
    manager.execute(p, &ctx).unwrap();
    let result = manager.discover("fixture", Kind::Tools, &ctx, |p| p.permit());
    match mode {
        0 => {
            assert_eq!(result.unwrap().items.len(), 2);
            let stale = manager
                .prepare_tool_call("fixture", "read", Map::new())
                .unwrap()
                .permit()
                .unwrap();
            let p = manager
                .prepare_notifications("fixture")
                .unwrap()
                .permit()
                .unwrap();
            assert!(matches!(
                manager.execute(p, &ctx).unwrap(),
                Outcome::Events { supported: true }
            ));
            assert!(manager.catalog("fixture", Kind::Tools).unwrap().is_none());
            assert!(matches!(
                manager.execute(stale, &ctx),
                Err(Error::Discovery(DiscoveryError::Stale))
            ));
            assert_eq!(
                manager.pop_notification().unwrap().method,
                "notifications/prompts/list_changed"
            );
        }
        1 => assert!(matches!(
            result,
            Err(Error::Discovery(DiscoveryError::Stale))
        )),
        _ => {
            assert!(result.is_err());
            assert_eq!(manager.statuses()[0].phase, "closed");
        }
    }
    manager.close(&ctx).unwrap();
    worker.join().unwrap();
}
