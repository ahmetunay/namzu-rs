use crate::{
    Error, Limits,
    types::{
        Implementation, InitializeParams, InitializeResult, PREFERRED_VERSION, SUPPORTED_VERSIONS,
    },
    wire::{self, Id, METHOD_NOT_FOUND, Message, RpcError},
};
use namzu_core::{AgentError, TurnContext};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    New,
    Initializing,
    AwaitingInitialized,
    Ready,
    Closed,
}
struct Pending {
    method: String,
    context: TurnContext,
}
#[derive(Debug, PartialEq)]
pub struct Expired {
    pub id: Id,
    pub cause: AgentError,
    pub notification: Option<Message>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Abandoned {
    pub id: Id,
    pub method: String,
}
#[derive(Debug, PartialEq)]
pub enum Incoming {
    Initialized(InitializeResult),
    Reply {
        id: Id,
        method: String,
        result: Result<Map<String, Value>, RpcError>,
    },
    Notification {
        method: String,
        params: Option<Map<String, Value>>,
    },
    /// Caller writes this bounded response; no server-initiated host effect occurs.
    Respond(Message),
    IgnoredLateReply,
    Expired(Expired),
}
/// One connection generation only. Never reuse this object for a reconnect.
/// Host owns transport writes/cleanup and polls expire() during network waits.
pub struct ClientSession {
    limits: Limits,
    info: Implementation,
    phase: Phase,
    issued: usize,
    pending: BTreeMap<Id, Pending>,
    completed: BTreeSet<Id>,
    cancelled: BTreeSet<Id>,
    server: Option<InitializeResult>,
}
impl ClientSession {
    pub fn new(info: Implementation, limits: Limits) -> Result<Self, Error> {
        limits.validate()?;
        wire::validate_metadata(&info.extra, &limits)?;
        if !info.valid() || info.extra.contains_key("name") || info.extra.contains_key("version") {
            return Err(Error::Initialize);
        }
        Ok(Self {
            limits,
            info,
            phase: Phase::New,
            issued: 0,
            pending: BTreeMap::new(),
            completed: BTreeSet::new(),
            cancelled: BTreeSet::new(),
            server: None,
        })
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn server(&self) -> Option<&InitializeResult> {
        self.server.as_ref()
    }
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
    pub fn encode(&self, message: &Message) -> Result<Vec<u8>, Error> {
        wire::encode(message, &self.limits)
    }
    pub fn initialize(&mut self, context: &TurnContext) -> Result<Message, Error> {
        if self.phase != Phase::New {
            return Err(Error::Phase);
        }
        let params = serde_json::to_value(InitializeParams {
            protocol_version: PREFERRED_VERSION.into(),
            capabilities: Map::new(),
            client_info: self.info.clone(),
        })
        .map_err(|_| Error::Initialize)?;
        let Value::Object(params) = params else {
            return Err(Error::Initialize);
        };
        let message = self.admit("initialize", Some(params), context)?;
        self.phase = Phase::Initializing;
        Ok(message)
    }
    /// Return the notification without advancing state. The transport MUST call
    /// mark_initialized_sent only after the complete notification has been written.
    pub fn initialized_notification(&self) -> Result<Message, Error> {
        if self.phase != Phase::AwaitingInitialized {
            return Err(Error::Phase);
        }
        Ok(Message::Notification {
            method: "notifications/initialized".into(),
            params: None,
        })
    }
    pub fn mark_initialized_sent(&mut self) -> Result<(), Error> {
        if self.phase != Phase::AwaitingInitialized {
            return Err(Error::Phase);
        }
        self.phase = Phase::Ready;
        Ok(())
    }
    pub fn request(
        &mut self,
        method: &str,
        params: Option<Map<String, Value>>,
        context: &TurnContext,
    ) -> Result<Message, Error> {
        if self.phase != Phase::Ready {
            return Err(Error::Phase);
        }
        if !self.enabled(method) || params.as_ref().is_some_and(|p| p.contains_key("task")) {
            return Err(Error::Capability);
        }
        self.admit(method, params, context)
    }
    fn enabled(&self, method: &str) -> bool {
        let Some(server) = &self.server else {
            return false;
        };
        let caps = &server.capabilities;
        match method {
            "ping" => true,
            "tools/list" | "tools/call" => caps.tools.is_some(),
            "prompts/list" | "prompts/get" => caps.prompts.is_some(),
            "resources/list" | "resources/templates/list" | "resources/read" => {
                caps.resources.is_some()
            }
            "resources/subscribe" | "resources/unsubscribe" => caps
                .resources
                .as_ref()
                .is_some_and(|r| r.subscribe == Some(true)),
            "logging/setLevel" => caps.logging.is_some(),
            "completion/complete" => caps.completions.is_some(),
            _ => false,
        }
    }
    fn admit(
        &mut self,
        method: &str,
        params: Option<Map<String, Value>>,
        context: &TurnContext,
    ) -> Result<Message, Error> {
        context.remaining()?;
        if self.issued >= self.limits.requests || self.pending.len() >= self.limits.pending {
            return Err(Error::RequestLimit);
        }
        let id = Id::String(format!("mcp-{}", self.issued + 1));
        let message = Message::Request {
            id: id.clone(),
            method: method.into(),
            params,
        };
        wire::encode(&message, &self.limits)?;
        let child = context.child(context.remaining()?)?;
        self.pending.insert(
            id,
            Pending {
                method: method.into(),
                context: child,
            },
        );
        self.issued += 1;
        Ok(message)
    }
    pub fn receive(&mut self, bytes: &[u8]) -> Result<Incoming, Error> {
        if matches!(self.phase, Phase::New | Phase::Closed) {
            return Err(Error::Phase);
        }
        let message = wire::decode(bytes, &self.limits)?;
        let (id, result) = match message {
            Message::Success { id, result } => (id, Ok(result)),
            Message::Failure {
                id: Some(id),
                error,
            } => (id, Err(error)),
            Message::Failure { id: None, .. } => return Err(Error::ResponseId),
            Message::Request { id, method, .. } => {
                return Ok(Incoming::Respond(if method == "ping" {
                    Message::Success {
                        id,
                        result: Map::new(),
                    }
                } else {
                    Message::Failure {
                        id: Some(id),
                        error: RpcError {
                            code: METHOD_NOT_FOUND,
                            message: "Client method is not enabled".into(),
                            data: None,
                        },
                    }
                }));
            }
            Message::Notification { method, params } => {
                // Direction matters: peer cancellation references its own requests,
                // never our outbound requests, even when IDs happen to match.
                return Ok(Incoming::Notification { method, params });
            }
        };
        if self.cancelled.contains(&id) {
            return Ok(Incoming::IgnoredLateReply);
        }
        let pending = self.pending.get(&id).ok_or(Error::ResponseId)?;
        if let Err(cause) = pending.context.remaining() {
            return Ok(Incoming::Expired(self.retire(&id, cause)));
        }
        let method = pending.method.clone();
        if method == "initialize" {
            let initialized = match result {
                Ok(result) => serde_json::from_value::<InitializeResult>(Value::Object(result))
                    .ok()
                    .filter(|r| {
                        SUPPORTED_VERSIONS.contains(&r.protocol_version.as_str())
                            && r.server_info.valid()
                    }),
                Err(_) => None,
            };
            let Some(server) = initialized else {
                self.phase = Phase::Closed;
                return Err(Error::Initialize);
            };
            self.pending.remove(&id);
            self.completed.insert(id);
            self.server = Some(server.clone());
            self.phase = Phase::AwaitingInitialized;
            return Ok(Incoming::Initialized(server));
        }
        self.pending.remove(&id);
        self.completed.insert(id.clone());
        Ok(Incoming::Reply { id, method, result })
    }
    fn retire(&mut self, id: &Id, cause: AgentError) -> Expired {
        let pending = self.pending.remove(id).expect("known pending request");
        self.cancelled.insert(id.clone());
        let notification = if pending.method == "initialize" {
            self.phase = Phase::Closed;
            None
        } else {
            Some(Message::Notification {
                method: "notifications/cancelled".into(),
                params: Some(json!({"requestId":id}).as_object().expect("object").clone()),
            })
        };
        Expired {
            id: id.clone(),
            cause,
            notification,
        }
    }
    /// Cancel means "stop using the result", never proof of remote rollback/stop.
    /// Notifications must be written best-effort by the future transport layer.
    pub fn cancel(&mut self, id: &Id) -> Result<Option<Message>, Error> {
        let Some(pending) = self.pending.get(id) else {
            return Ok(None);
        };
        if pending.method == "initialize" {
            return Err(Error::CancelInitialize);
        }
        Ok(self.retire(id, AgentError::Cancelled).notification)
    }
    /// Absolute parent deadlines are never extended by notifications/progress.
    pub fn expire(&mut self) -> Vec<Expired> {
        let expired: Vec<_> = self
            .pending
            .iter()
            .filter_map(|(id, p)| p.context.remaining().err().map(|e| (id.clone(), e)))
            .collect();
        expired
            .into_iter()
            .map(|(id, cause)| self.retire(&id, cause))
            .collect()
    }
    /// Connection loss fences this session. All drained requests may have reached
    /// the peer; caller must inspect effects, never replay these automatically.
    pub fn close(&mut self) -> Vec<Abandoned> {
        self.phase = Phase::Closed;
        std::mem::take(&mut self.pending)
            .into_iter()
            .map(|(id, p)| Abandoned {
                id,
                method: p.method,
            })
            .collect()
    }
}
