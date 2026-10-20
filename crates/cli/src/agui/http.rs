//! Small bounded one-request HTTP/1.1 transport. No chunked request bodies or pipelining.
use namzu_ag_ui::MAX_INPUT;
use serde_json::json;
use std::{
    io::{Read, Write},
    net::TcpStream,
    time::{Duration, Instant},
};
pub struct Connection {
    pub socket: TcpStream,
    bytes: Vec<u8>,
    deadline: Instant,
}
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}
impl Request {
    pub fn header(&self, key: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}
impl Connection {
    pub fn new(socket: TcpStream) -> Self {
        Self {
            socket,
            bytes: vec![],
            deadline: Instant::now() + Duration::from_secs(5),
        }
    }
    pub fn poll(&mut self) -> Result<Option<Request>, u16> {
        if Instant::now() >= self.deadline {
            return Err(408);
        }
        let mut block = [0; 8192];
        match self.socket.read(&mut block) {
            Ok(0) => return Err(400),
            Ok(n) => {
                self.bytes.extend_from_slice(&block[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => return Err(400),
        }
        let Some(end) = self
            .bytes
            .windows(4)
            .position(|v| v == b"\r\n\r\n")
            .map(|n| n + 4)
        else {
            return if self.bytes.len() > 16384 {
                Err(431)
            } else {
                Ok(None)
            };
        };
        if end > 16384 {
            return Err(431);
        }
        let text = std::str::from_utf8(&self.bytes[..end]).map_err(|_| 400u16)?;
        let mut lines = text.split("\r\n");
        let mut first = lines.next().unwrap_or_default().split_whitespace();
        let method = first.next().ok_or(400u16)?.to_owned();
        let path = first.next().ok_or(400u16)?.to_owned();
        if first.next() != Some("HTTP/1.1")
            || first.next().is_some()
            || !path.starts_with('/')
            || path.contains('?')
            || path.contains('#')
        {
            return Err(400);
        }
        let mut headers = vec![];
        for line in lines.filter(|s| !s.is_empty()) {
            let (key, value) = line.split_once(':').ok_or(400u16)?;
            if key.is_empty()
                || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                || value.chars().any(|c| c.is_control() && c != '\t')
            {
                return Err(400);
            }
            let key = key.to_ascii_lowercase();
            if headers.iter().any(|(k, _)| k == &key) {
                return Err(400);
            }
            headers.push((key, value.trim().to_owned()));
        }
        let get = |name: &str| {
            headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
        };
        if get("transfer-encoding").is_some() || get("expect").is_some() {
            return Err(400);
        }
        let length = match get("content-length") {
            Some(v) if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) => {
                v.parse::<usize>().map_err(|_| 413u16)?
            }
            None if method == "GET" => 0,
            _ => return Err(411),
        };
        if length > MAX_INPUT {
            return Err(413);
        }
        let expected = end + length;
        if self.bytes.len() > expected {
            return Err(400);
        }
        if self.bytes.len() < expected {
            return Ok(None);
        }
        Ok(Some(Request {
            method,
            path,
            headers,
            body: self.bytes[end..].to_vec(),
        }))
    }
}
pub fn response(socket: &mut TcpStream, status: u16, code: &str) {
    let body = json!({"status":if status<300{"ok"}else{"error"},"code":code}).to_string();
    let bytes = format!(
        "HTTP/1.1 {status} Result\r\nConnection: close\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let _ = write_until(
        socket,
        bytes.as_bytes(),
        Instant::now() + Duration::from_millis(200),
        || true,
    );
}
pub fn write_until(
    socket: &mut TcpStream,
    mut bytes: &[u8],
    deadline: Instant,
    alive: impl Fn() -> bool,
) -> std::io::Result<()> {
    while !bytes.is_empty() {
        if Instant::now() >= deadline || !alive() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "AG-UI client disconnected or stalled",
            ));
        }
        match socket.write(bytes) {
            Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
            Ok(n) => bytes = &bytes[n..],
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    fn parse(raw: &[u8]) -> Result<Option<Request>, u16> {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(l.local_addr().unwrap()).unwrap();
        let (server, _) = l.accept().unwrap();
        server.set_nonblocking(true).unwrap();
        client.write_all(raw).unwrap();
        let mut c = Connection::new(server);
        for _ in 0..100 {
            match c.poll() {
                Ok(None) => std::thread::sleep(Duration::from_millis(1)),
                v => return v,
            }
        }
        Ok(None)
    }
    #[test]
    fn rejects_ambiguous_framing_and_absolute_targets() {
        for raw in [
            "POST /agent HTTP/1.1\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n",
            "POST /agent HTTP/1.1\r\nContent-Length: 0\r\nTransfer-Encoding: chunked\r\n\r\n",
            "POST http://host/agent HTTP/1.1\r\nContent-Length: 0\r\n\r\n",
            "POST /agent HTTP/1.1\r\nContent-Length: 0\r\n\r\nx",
        ] {
            assert!(parse(raw.as_bytes()).is_err());
        }
    }
    #[test]
    fn request_body_is_bounded_and_fragmented_body_waits() {
        assert!(parse(b"POST /agent HTTP/1.1\r\nContent-Length: 9999999\r\n\r\n").is_err());
        assert!(
            parse(b"POST /agent HTTP/1.1\r\nContent-Length: 2\r\n\r\nx")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            parse(b"POST /agent HTTP/1.1\r\nContent-Length: 2\r\n\r\n{}")
                .unwrap()
                .unwrap()
                .body,
            b"{}"
        );
    }
}
