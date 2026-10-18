//! Streamable HTTP (2025-11-25 / 2025-06-18), with bounded JSON/SSE replies.
//! Explicit single-flight client: no automatic retries, redirects, login or resume.
mod sse;
use crate::{
    Abandoned, ClientSession, Incoming, Limits, Message, Phase,
    types::{Implementation, InitializeResult},
};
use namzu_core::{AgentError, TurnContext};
use reqwest::{
    Method, Response, Url,
    header::{ACCEPT, CONTENT_TYPE, HeaderValue},
};
use serde_json::{Map, Value};
use std::{collections::VecDeque, future::Future, sync::Arc, time::Duration};

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("invalid MCP HTTP endpoint or transport settings")]
    Settings,
    #[error("MCP HTTP network operation failed")]
    Network,
    #[error("MCP HTTP status {0}")]
    Status(u16),
    #[error("MCP HTTP session expired; explicit new connection required")]
    SessionExpired,
    #[error("invalid MCP HTTP response headers")]
    Headers,
    #[error("MCP HTTP stream ended without a complete response")]
    Stream,
    #[error("MCP HTTP resource limit exceeded")]
    Limit,
    #[error(transparent)]
    Protocol(#[from] crate::Error),
    #[error("MCP HTTP operation ended: {0}")]
    Context(#[from] AgentError),
}
/// Endpoint must be HTTPS, or explicit literal loopback HTTP for local servers.
/// Deliberately no Debug implementation: URLs and bearer tokens can be sensitive.
pub struct Spec {
    pub endpoint: String,
    pub bearer: Option<String>,
    pub limits: Limits,
}
impl Spec {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            bearer: None,
            limits: Limits::default(),
        }
    }
}
/// No I/O on Drop. The host explicitly calls close with its own cleanup deadline.
/// Errors fence this generation and retain uncertain requests in abandoned().
pub struct Client {
    session: ClientSession,
    http: reqwest::Client,
    runtime: Arc<tokio::runtime::Runtime>,
    endpoint: Url,
    token: Option<HeaderValue>,
    session_id: Option<HeaderValue>,
    limit: usize,
    limits: Limits,
    events: VecDeque<(Incoming, usize)>,
    event_bytes: usize,
    uncertain: Vec<Abandoned>,
    listening: Option<Stream>,
}
async fn wait<T>(
    ctx: &TurnContext,
    f: impl Future<Output = Result<T, reqwest::Error>>,
) -> Result<T, Error> {
    let remaining = ctx.remaining()?;
    tokio::select! { biased;
        cause = async { loop { if let Err(e)=ctx.remaining() { return e; } tokio::time::sleep(Duration::from_millis(10)).await; } } => Err(Error::Context(cause)),
        result = tokio::time::timeout(remaining, f) => {ctx.remaining()?; result.map_err(|_|Error::Context(AgentError::Timeout))?.map_err(|_|Error::Network)}
    }
}
impl Client {
    pub fn connect(spec: Spec, info: Implementation, ctx: &TurnContext) -> Result<Self, Error> {
        ctx.remaining()?;
        let endpoint = Url::parse(&spec.endpoint).map_err(|_| Error::Settings)?;
        let loopback = endpoint.host_str().is_some_and(|host| {
            host.trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
        });
        if !(endpoint.scheme() == "https" || endpoint.scheme() == "http" && loopback)
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(Error::Settings);
        }
        let token = spec
            .bearer
            .map(|token| {
                if token.is_empty() || !token.bytes().all(|b| b > 32 && b < 127) {
                    return Err(Error::Settings);
                }
                let mut h = HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|_| Error::Settings)?;
                h.set_sensitive(true);
                Ok(h)
            })
            .transpose()?;
        let session = ClientSession::new(info, spec.limits.clone())?;
        let runtime = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| Error::Settings)?,
        );
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .map_err(|_| Error::Settings)?;
        let mut client = Self {
            session,
            http,
            runtime,
            endpoint,
            token,
            session_id: None,
            limit: spec.limits.message_bytes,
            limits: spec.limits,
            events: VecDeque::new(),
            event_bytes: 0,
            uncertain: vec![],
            listening: None,
        };
        let message = client.session.initialize(ctx)?;
        let result = (|| {
            client.exchange(message, ctx)?;
            let ack = client.session.initialized_notification()?;
            client.accepted(&ack, ctx)?;
            client.session.mark_initialized_sent()?;
            Ok(())
        })();
        client.fence(result)?;
        Ok(client)
    }
    pub fn server(&self) -> Option<&InitializeResult> {
        self.session.server()
    }
    pub fn phase(&self) -> Phase {
        self.session.phase()
    }
    pub fn abandoned(&self) -> &[Abandoned] {
        &self.uncertain
    }
    /// Notifications are inert data. The bounded queue must be drained by the host.
    pub fn pop_event(&mut self) -> Option<Incoming> {
        self.events.pop_front().map(|(event, size)| {
            self.event_bytes -= size;
            event
        })
    }
    fn fence<T>(&mut self, result: Result<T, Error>) -> Result<T, Error> {
        if result.is_err() {
            self.uncertain.extend(self.session.close());
            self.listening = None;
        }
        result
    }
    pub fn call(
        &mut self,
        method: &str,
        params: Option<Map<String, Value>>,
        ctx: &TurnContext,
    ) -> Result<Incoming, Error> {
        let message = self.session.request(method, params, ctx)?;
        let pending = match &message {
            Message::Request { id, method, .. } => Abandoned {
                id: id.clone(),
                method: method.clone(),
            },
            _ => unreachable!(),
        };
        let result = self.exchange(message, ctx);
        let result = self.fence(result);
        if result.is_err() && !self.uncertain.iter().any(|p| p.id == pending.id) {
            self.uncertain.push(pending);
        }
        result
    }
    fn send(
        &self,
        method: Method,
        body: Option<Vec<u8>>,
        ctx: &TurnContext,
    ) -> Result<Response, Error> {
        ctx.remaining()?;
        let accept = if method == Method::GET {
            "text/event-stream"
        } else {
            "application/json, text/event-stream"
        };
        let mut request = self
            .http
            .request(method, self.endpoint.clone())
            .header(ACCEPT, accept);
        if let Some(token) = &self.token {
            request = request.header(reqwest::header::AUTHORIZATION, token.clone());
        }
        if let Some(id) = &self.session_id {
            request = request.header("MCP-Session-Id", id.clone());
        }
        if let Some(server) = self.session.server() {
            request = request.header("MCP-Protocol-Version", &server.protocol_version);
        }
        if let Some(body) = body {
            request = request.header(CONTENT_TYPE, "application/json").body(body);
        }
        let response = self.runtime.block_on(wait(ctx, request.send()))?;
        if response.status().as_u16() == 404 && self.session_id.is_some() {
            return Err(Error::SessionExpired);
        }
        if !response.status().is_success() {
            return Err(Error::Status(response.status().as_u16()));
        }
        if response.headers().get_all("MCP-Session-Id").iter().count() > 1 {
            return Err(Error::Headers);
        }
        if let Some(id) = response.headers().get("MCP-Session-Id")
            && (self.session_id.as_ref().is_some_and(|old| old != id)
                || self.session_id.is_none() && self.session.phase() != Phase::Initializing)
        {
            return Err(Error::Headers);
        }
        Ok(response)
    }
    fn chunk(&self, response: &mut Response, ctx: &TurnContext) -> Result<Option<Vec<u8>>, Error> {
        self.runtime
            .block_on(wait(ctx, response.chunk()))?
            .map(|b| {
                if b.len() > 16 * 1024 * 1024 {
                    Err(Error::Limit)
                } else {
                    Ok(b.to_vec())
                }
            })
            .transpose()
    }
    fn empty(&self, mut response: Response, ctx: &TurnContext) -> Result<(), Error> {
        while let Some(bytes) = self.chunk(&mut response, ctx)? {
            if !bytes.is_empty() {
                return Err(Error::Stream);
            }
        }
        Ok(())
    }
    fn accepted(&self, message: &Message, ctx: &TurnContext) -> Result<(), Error> {
        let response = self.send(Method::POST, Some(self.session.encode(message)?), ctx)?;
        if response.status().as_u16() != 202 {
            return Err(Error::Status(response.status().as_u16()));
        }
        self.empty(response, ctx)
    }
    fn incoming(&mut self, bytes: &[u8], ctx: &TurnContext) -> Result<Option<Incoming>, Error> {
        let incoming = self.session.receive(bytes)?;
        match incoming {
            Incoming::Respond(message) => {
                self.accepted(&message, ctx)?;
                Ok(None)
            }
            Incoming::Notification { .. } => {
                if self.events.len() >= 64
                    || self.event_bytes.saturating_add(bytes.len()) > 2 * 1024 * 1024
                {
                    return Err(Error::Limit);
                }
                self.event_bytes += bytes.len();
                self.events.push_back((incoming, bytes.len()));
                Ok(None)
            }
            Incoming::Expired(expired) => Err(Error::Context(expired.cause)),
            Incoming::IgnoredLateReply => Err(Error::Stream),
            other => Ok(Some(other)),
        }
    }
    fn exchange(&mut self, message: Message, ctx: &TurnContext) -> Result<Incoming, Error> {
        let initializing = self.session.phase() == Phase::Initializing;
        let mut response = self.send(Method::POST, Some(self.session.encode(&message)?), ctx)?;
        if response.status().as_u16() != 200 {
            return Err(Error::Status(response.status().as_u16()));
        }
        if initializing && let Some(id) = response.headers().get("MCP-Session-Id") {
            if id.as_bytes().is_empty()
                || id.as_bytes().len() > 1024
                || !id.as_bytes().iter().all(|b| *b >= 0x21 && *b <= 0x7e)
            {
                return Err(Error::Headers);
            }
            let mut id = id.clone();
            id.set_sensitive(true);
            self.session_id = Some(id);
        }
        if response.headers().get_all(CONTENT_TYPE).iter().count() != 1 {
            return Err(Error::Headers);
        }
        let content = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if content == "application/json" {
            let mut bytes = Vec::new();
            while let Some(chunk) = self.chunk(&mut response, ctx)? {
                if bytes.len().saturating_add(chunk.len()) > self.limit {
                    return Err(Error::Limit);
                }
                bytes.extend(chunk);
            }
            if !matches!(
                crate::wire::decode(&bytes, &self.limits)?,
                Message::Success { .. } | Message::Failure { .. }
            ) {
                return Err(Error::Stream);
            }
            return self.incoming(&bytes, ctx)?.ok_or(Error::Stream);
        }
        if content != "text/event-stream" {
            return Err(Error::Headers);
        }
        self.stream(response, ctx)
    }
    /// Continues a retained optional GET stream, returning one notification.
    /// 405 means unsupported. Unread events are preserved between calls.
    /// No background task, reconnect, resume, or implicit new stream after EOF.
    pub fn listen(&mut self, ctx: &TurnContext) -> Result<Option<Incoming>, Error> {
        ctx.remaining()?;
        if self.phase() != Phase::Ready {
            return Err(crate::Error::Phase.into());
        }
        if let Some(event) = self.pop_event() {
            return Ok(Some(event));
        }
        let result = (|| {
            let mut stream = match self.listening.take() {
                Some(stream) => stream,
                None => {
                    let response = match self.send(Method::GET, None, ctx) {
                        Err(Error::Status(405)) => return Ok(None),
                        other => other?,
                    };
                    if response.status().as_u16() != 200 || !is_sse(&response) {
                        return Err(Error::Headers);
                    }
                    Stream::new(response, self.limit)
                }
            };
            loop {
                let data = self.packet(&mut stream, ctx)?;
                match crate::wire::decode(&data, &self.limits)? {
                    Message::Request { .. } | Message::Notification { .. } => {}
                    _ => return Err(Error::Stream),
                }
                self.incoming(&data, ctx)?;
                if let Some(event) = self.pop_event() {
                    self.listening = Some(stream);
                    return Ok(Some(event));
                }
            }
        })();
        self.fence(result)
    }
    fn packet(&self, stream: &mut Stream, ctx: &TurnContext) -> Result<Vec<u8>, Error> {
        loop {
            ctx.remaining()?;
            for (index, byte) in stream.buffer.by_ref().enumerate() {
                if index % 4096 == 0 {
                    ctx.remaining()?;
                }
                if let Some(data) = stream.decoder.push(byte)? {
                    stream.count += 1;
                    if stream.count > 4096 {
                        return Err(Error::Limit);
                    }
                    return Ok(data);
                }
            }
            let chunk = self
                .chunk(&mut stream.response, ctx)?
                .ok_or(Error::Stream)?;
            stream.total = stream.total.saturating_add(chunk.len());
            if stream.total > 16 * 1024 * 1024 {
                return Err(Error::Limit);
            }
            stream.buffer = chunk.into_iter();
        }
    }
    fn stream(&mut self, response: Response, ctx: &TurnContext) -> Result<Incoming, Error> {
        let mut stream = Stream::new(response, self.limit);
        loop {
            let data = self.packet(&mut stream, ctx)?;
            if let Some(incoming) = self.incoming(&data, ctx)? {
                return Ok(incoming);
            }
        }
    }
    /// Explicit best-effort remote DELETE. 405 means remote termination unsupported.
    /// Does not run implicitly on Drop and does not retry any request.
    pub fn close(&mut self, ctx: &TurnContext) -> Result<(), Error> {
        self.uncertain.extend(self.session.close());
        self.listening = None;
        if self.session_id.is_none() {
            return Ok(());
        }
        let result = match self.send(Method::DELETE, None, ctx) {
            Ok(response) => self.empty(response, ctx),
            Err(Error::Status(405) | Error::SessionExpired) => Ok(()),
            Err(e) => Err(e),
        };
        self.session_id = None;
        result
    }
}

fn is_sse(response: &Response) -> bool {
    if response.headers().get_all(CONTENT_TYPE).iter().count() != 1 {
        return false;
    }
    response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.split(';').next())
        .is_some_and(|h| h.trim().eq_ignore_ascii_case("text/event-stream"))
}
struct Stream {
    response: Response,
    decoder: sse::Decoder,
    buffer: std::vec::IntoIter<u8>,
    total: usize,
    count: usize,
}
impl Stream {
    fn new(response: Response, limit: usize) -> Self {
        Self {
            response,
            decoder: sse::Decoder::new(limit),
            buffer: vec![].into_iter(),
            total: 0,
            count: 0,
        }
    }
}
