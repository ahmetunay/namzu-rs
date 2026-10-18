use super::*;
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream},
    thread,
    time::{Duration, Instant},
};
pub(super) struct Listener {
    socket: TcpListener,
}
impl Listener {
    pub fn bind(port: u16) -> Result<Self, Error> {
        let socket = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
            .map_err(|_| Error::Callback)?;
        socket.set_nonblocking(true).map_err(|_| Error::Callback)?;
        Ok(Self { socket })
    }
    pub fn redirect(&self) -> String {
        format!(
            "http://{}/oauth/callback",
            self.socket.local_addr().expect("bound listener")
        )
    }
    pub fn wait(
        self,
        state: &str,
        metadata: &Metadata,
        ctx: &TurnContext,
    ) -> Result<String, Error> {
        for _ in 0..32 {
            let (mut socket, peer) = loop {
                ctx.remaining()?;
                match self.socket.accept() {
                    Ok(s) => break s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(_) => return Err(Error::Callback),
                }
            };
            if !peer.ip().is_loopback() {
                continue;
            }
            let result = read(&mut socket, ctx)
                .and_then(|request| parse(&request, &self.redirect(), state, metadata));
            let valid = result.as_ref().is_ok_and(|v| v.is_some());
            let text = if valid {
                "200 OK\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Type: text/plain\r\nContent-Length: 31\r\n\r\nAuthorization received. Return."
            } else {
                "400 Bad Request\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Length: 0\r\n\r\n"
            };
            let _ = socket.write_all(format!("HTTP/1.1 {text}").as_bytes());
            match result {
                Ok(Some(code)) => return Ok(code),
                Err(Error::Context(e)) => return Err(Error::Context(e)),
                Err(Error::Denied) => return Err(Error::Denied),
                _ => {}
            }
        }
        Err(Error::Callback)
    }
}
fn read(socket: &mut TcpStream, ctx: &TurnContext) -> Result<String, Error> {
    socket.set_nonblocking(true).map_err(|_| Error::Callback)?;
    let until = Instant::now() + Duration::from_secs(2);
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 1024];
    while !bytes.ends_with(b"\r\n\r\n") {
        ctx.remaining()?;
        if Instant::now() >= until {
            return Err(Error::Callback);
        }
        match socket.read(&mut buffer) {
            Ok(0) => return Err(Error::Callback),
            Ok(n) => {
                bytes.extend_from_slice(&buffer[..n]);
                if bytes.len() > 8192 {
                    return Err(Error::Limit);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5))
            }
            Err(_) => return Err(Error::Callback),
        }
    }
    String::from_utf8(bytes).map_err(|_| Error::Callback)
}
fn parse(
    request: &str,
    redirect: &str,
    state: &str,
    m: &Metadata,
) -> Result<Option<String>, Error> {
    let mut lines = request.split("\r\n");
    let mut words = lines.next().ok_or(Error::Callback)?.split(' ');
    if words.next() != Some("GET") {
        return Err(Error::Callback);
    }
    let path = words.next().ok_or(Error::Callback)?;
    if words.next() != Some("HTTP/1.1")
        || words.next().is_some()
        || !path.starts_with("/oauth/callback?")
    {
        return Err(Error::Callback);
    }
    let base = Url::parse(redirect).map_err(|_| Error::Callback)?;
    let mut hosts = 0;
    for line in lines.filter(|s| !s.is_empty()) {
        let (key, value) = line.split_once(':').ok_or(Error::Callback)?;
        if key.eq_ignore_ascii_case("host") {
            hosts += 1;
            if value.trim() != format!("127.0.0.1:{}", base.port().ok_or(Error::Callback)?) {
                return Err(Error::Callback);
            }
        }
    }
    if hosts != 1 {
        return Err(Error::Callback);
    }
    let url = base.join(path).map_err(|_| Error::Callback)?;
    let mut pairs = std::collections::BTreeMap::new();
    for (key, value) in url.query_pairs() {
        if pairs.insert(key.into_owned(), value.into_owned()).is_some() {
            return Err(Error::Callback);
        }
    }
    if pairs.get("state").map(String::as_str) != Some(state) {
        return Err(Error::Callback);
    }
    if pairs.contains_key("error") {
        return Err(Error::Denied);
    }
    if pairs.get("iss").is_some_and(|s| s != &m.issuer)
        || m.require_iss && !pairs.contains_key("iss")
    {
        return Err(Error::Callback);
    }
    let code = pairs
        .remove("code")
        .filter(|s| token(s) && s.len() <= 4096)
        .ok_or(Error::Callback)?;
    Ok(Some(code))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn meta() -> Metadata {
        Metadata {
            issuer: "https://issuer.test".into(),
            authorization_endpoint: "https://issuer.test/auth".into(),
            token_endpoint: "https://issuer.test/token".into(),
            require_iss: true,
        }
    }
    fn request(query: &str, host: &str) -> String {
        format!("GET /oauth/callback?{query} HTTP/1.1\r\nHost: {host}\r\n\r\n")
    }
    #[test]
    fn callback_requires_exact_state_host_and_issuer() {
        let redirect = "http://127.0.0.1:4444/oauth/callback";
        let q = "state=correct&code=fixture&iss=https%3A%2F%2Fissuer.test";
        assert_eq!(
            parse(&request(q, "127.0.0.1:4444"), redirect, "correct", &meta()).unwrap(),
            Some("fixture".into())
        );
        for q in [
            "state=wrong&code=fixture&iss=https%3A%2F%2Fissuer.test",
            "state=correct&state=correct&code=fixture",
            "state=correct&code=fixture",
            "state=correct&code=fixture&iss=https%3A%2F%2Fevil.test",
        ] {
            assert!(parse(&request(q, "127.0.0.1:4444"), redirect, "correct", &meta()).is_err())
        }
        assert!(parse(&request(q, "evil.test"), redirect, "correct", &meta()).is_err());
    }
    #[test]
    fn callback_error_is_terminal_only_for_its_own_state() {
        let r = "http://127.0.0.1:4444/oauth/callback";
        assert!(matches!(
            parse(
                &request("state=correct&error=access_denied", "127.0.0.1:4444"),
                r,
                "correct",
                &meta()
            ),
            Err(Error::Denied)
        ));
        assert!(matches!(
            parse(
                &request("state=wrong&error=access_denied", "127.0.0.1:4444"),
                r,
                "correct",
                &meta()
            ),
            Err(Error::Callback)
        ));
    }
    #[test]
    fn callback_listener_cancellation_is_bounded() {
        let cancel = namzu_core::Cancellation::default();
        let ctx = TurnContext::new(Duration::from_secs(300), cancel.clone()).unwrap();
        cancel.cancel();
        assert!(matches!(
            Listener::bind(0).unwrap().wait("state", &meta(), &ctx),
            Err(Error::Context(AgentError::Cancelled))
        ));
    }
}
