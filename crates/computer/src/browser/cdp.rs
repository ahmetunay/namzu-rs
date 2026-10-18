//! Local-only sequential CDP transport. Endpoint discovery never follows URLs.
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::Duration,
};
use tungstenite::{Message, WebSocket, protocol::WebSocketConfig};
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const FRAME_LIMIT: usize = 4 * 1024 * 1024;
pub struct Cdp {
    socket: WebSocket<TcpStream>,
    next_id: u64,
    events: VecDeque<Value>,
    event_bytes: usize,
}
impl Cdp {
    pub fn connect() -> Result<Self> {
        let until = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match Self::at(9222) {
                Ok(c) => return Ok(c),
                Err(e) if std::time::Instant::now() >= until => return Err(e),
                Err(_) => std::thread::sleep(Duration::from_millis(25)),
            }
        }
    }
    fn at(port: u16) -> Result<Self> {
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        write!(
            stream,
            "GET /json/list HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
        )?;
        let mut bytes = Vec::new();
        let split = loop {
            if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                break end;
            }
            if bytes.len() > 8192 {
                return Err("CDP HTTP header limit".into());
            }
            let mut buffer = [0u8; 1024];
            let count = stream.read(&mut buffer)?;
            if count == 0 {
                return Err("incomplete CDP HTTP header".into());
            }
            bytes.extend_from_slice(&buffer[..count]);
        };
        let header = std::str::from_utf8(&bytes[..split])?;
        if !header.starts_with("HTTP/1.1 200 ") {
            return Err("CDP HTTP status".into());
        }
        let mut lengths = Vec::new();
        for line in header.lines().skip(1) {
            let (key, value) = line.split_once(':').ok_or("invalid CDP HTTP header")?;
            if key.eq_ignore_ascii_case("transfer-encoding") {
                return Err("unsupported CDP HTTP encoding".into());
            }
            if key.eq_ignore_ascii_case("content-length") {
                lengths.push(value.trim().parse::<usize>()?);
            }
        }
        if lengths.len() != 1 || lengths[0] > 65536 || split > 8192 {
            return Err("CDP discovery length limit".into());
        }
        let end = split + 4 + lengths[0];
        if bytes.len() > end {
            return Err("extra CDP HTTP bytes".into());
        }
        let received = bytes.len();
        bytes.resize(end, 0);
        // Chromium keeps HTTP alive even with Connection: close. Never wait for EOF.
        stream.read_exact(&mut bytes[received..])?;
        let targets: Vec<Value> = serde_json::from_slice(&bytes[split + 4..])?;
        // Only one page is controlled. Extra popups/tabs are ambiguous and rejected.
        let pages: Vec<_> = targets.iter().filter(|t| t["type"] == "page").collect();
        if pages.len() != 1 {
            return Err("exactly one browser page required".into());
        }
        let endpoint = pages[0]["webSocketDebuggerUrl"]
            .as_str()
            .ok_or("CDP endpoint missing")?;
        let parsed = url::Url::parse(endpoint)?;
        if parsed.scheme() != "ws"
            || parsed.host_str() != Some("127.0.0.1")
            || parsed.port() != Some(port)
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || !parsed.path().starts_with("/devtools/page/")
            || endpoint.len() > 512
        {
            return Err("non-local CDP endpoint".into());
        }
        let stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(FRAME_LIMIT))
            .max_frame_size(Some(FRAME_LIMIT));
        let (socket, _) = tungstenite::client::client_with_config(endpoint, stream, Some(config))?;
        Ok(Self {
            socket,
            next_id: 1,
            events: VecDeque::new(),
            event_bytes: 0,
        })
    }
    fn receive(&mut self) -> Result<Value> {
        loop {
            match self.socket.read()? {
                Message::Text(s) => return Ok(serde_json::from_str(s.as_str())?),
                Message::Ping(_) | Message::Pong(_) => {}
                _ => return Err("unexpected CDP frame".into()),
            }
        }
    }
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).ok_or("CDP id overflow")?;
        self.socket.send(Message::Text(
            json!({"id":id,"method":method,"params":params})
                .to_string()
                .into(),
        ))?;
        loop {
            let value = self.receive()?;
            if value.get("id").is_some() {
                if value["id"] != id || value.get("error").is_some() {
                    return Err("CDP response rejected".into());
                }
                return value
                    .get("result")
                    .cloned()
                    .ok_or("CDP result missing".into());
            }
            self.event(value)?;
        }
    }
    fn event(&mut self, value: Value) -> Result<()> {
        let size = serde_json::to_vec(&value)?.len();
        if value["method"].as_str().is_none()
            || self.events.len() >= 256
            || size > 1024 * 1024 - self.event_bytes
        {
            return Err("invalid or flooded CDP events".into());
        }
        self.event_bytes += size;
        self.events.push_back(value);
        Ok(())
    }
    pub fn clear_events(&mut self) {
        self.events.clear();
        self.event_bytes = 0;
    }
    pub fn wait_loaded(&mut self, loader: &str) -> Result<()> {
        loop {
            if self.events.iter().any(|v| {
                v["method"] == "Page.lifecycleEvent"
                    && v["params"]["loaderId"] == loader
                    && v["params"]["name"] == "load"
            }) {
                return Ok(());
            }
            let value = self.receive()?;
            if value.get("id").is_some() {
                return Err("unsolicited CDP response".into());
            }
            self.event(value)?;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::TcpListener, thread};
    fn fixture(endpoint_host: &str, wrong_id: bool) -> (u16, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let host = endpoint_host.to_owned();
        let handle = thread::spawn(move || {
            let (mut http, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0u8; 1];
                http.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
                assert!(request.len() <= 8192);
            }
            let body=json!([{"type":"page","webSocketDebuggerUrl":format!("ws://{host}:{port}/devtools/page/fixture")}]).to_string();
            write!(
                http,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            if host != "127.0.0.1" {
                return;
            }
            let (stream, _) = listener.accept().unwrap();
            drop(http);
            let mut ws = tungstenite::accept(stream).unwrap();
            let Message::Text(text) = ws.read().unwrap() else {
                panic!()
            };
            let request: Value = serde_json::from_str(&text).unwrap();
            ws.send(Message::Text(
                json!({"method":"Page.lifecycleEvent","params":{"loaderId":"fresh","name":"load"}})
                    .to_string()
                    .into(),
            ))
            .unwrap();
            ws.send(Message::Text(json!({"id":if wrong_id {999} else {request["id"].as_u64().unwrap()},"result":{"ok":true}}).to_string().into())).unwrap();
        });
        (port, handle)
    }
    #[test]
    fn keep_alive_discovery_correlates_response_and_preserves_early_load_event() {
        let (port, h) = fixture("127.0.0.1", false);
        let mut c = Cdp::at(port).unwrap();
        assert_eq!(c.call("Page.navigate", json!({})).unwrap()["ok"], true);
        c.wait_loaded("fresh").unwrap();
        h.join().unwrap();
    }
    #[test]
    fn wrong_response_id_is_not_retried() {
        let (port, h) = fixture("127.0.0.1", true);
        assert!(
            Cdp::at(port)
                .unwrap()
                .call("Page.navigate", json!({}))
                .is_err()
        );
        h.join().unwrap();
    }
    #[test]
    fn rejects_remote_discovery_endpoint_before_connect() {
        let (port, h) = fixture("example.com", false);
        assert!(Cdp::at(port).is_err());
        h.join().unwrap();
    }
    #[test]
    fn notification_count_and_aggregate_bytes_are_bounded() {
        let (port, h) = fixture("127.0.0.1", false);
        let mut c = Cdp::at(port).unwrap();
        c.call("Page.enable", json!({})).unwrap();
        for _ in 0..255 {
            c.event(json!({"method":"fixture"})).unwrap();
        }
        assert!(c.event(json!({"method":"fixture"})).is_err());
        c.clear_events();
        assert!(
            c.event(json!({"method":"fixture","data":"x".repeat(1024*1024)}))
                .is_err()
        );
        assert_eq!(c.event_bytes, 0);
        h.join().unwrap();
    }
}
