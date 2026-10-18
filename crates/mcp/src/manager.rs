//! In-process, thread-affine server ownership behind one-operation admission.
use crate::{
    Id, Incoming, Limits, Message,
    config::{Decision, Error, Policy, Project, Registry, Resolved, Transport},
    types::Implementation,
};
use namzu_core::TurnContext;
use serde::Serialize;
use serde_json::{Map, Value};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
static OWNERS: AtomicU64 = AtomicU64::new(1);
/// Immutable intent bound to this manager and exact configuration snapshot.
/// Neither plans nor permissions are Clone/Serialize; argv/params are not Debug.
pub struct Plan {
    owner: u64,
    fingerprint: String,
    server: String,
    method: Option<String>,
    params: Option<Map<String, Value>>,
    decision: Decision,
}
impl Plan {
    pub fn decision(&self) -> Decision {
        self.decision
    }
    pub fn permit(self) -> Result<Permit, Error> {
        match self.decision {
            Decision::Allow => Ok(Permit(self)),
            Decision::Ask => Err(Error::ApprovalRequired),
            Decision::Deny => Err(Error::Denied),
        }
    }
    /// Host calls only after explicit approval for this immutable operation.
    /// Cannot override deny and cannot be reused for another payload/server.
    pub fn approve(self) -> Result<Permit, Error> {
        if self.decision == Decision::Deny {
            Err(Error::Denied)
        } else {
            Ok(Permit(self))
        }
    }
}
/// Consumed by execute; a grant cannot dispatch twice.
/// ```compile_fail
/// fn reuse(permission: namzu_mcp::manager::Permit) {
///     let _first = permission;
///     let _second = permission;
/// }
/// ```
pub struct Permit(Plan);
pub enum Outcome {
    Connected,
    Reply(Result<Map<String, Value>, crate::RpcError>),
}
#[derive(Serialize)]
pub struct Status {
    pub server: String,
    pub phase: &'static str,
}
enum Connection {
    #[cfg(unix)]
    Stdio(Box<crate::stdio::Client>),
    Http(Box<crate::http::Client>),
}
/// Registry mutations are locked during admitted effects. Revocation blocks new
/// admissions; an already admitted remote effect cannot be rolled back.
/// No automatic start, reconnect, retry, process shell or host/guest fallback.
pub struct Manager<'a> {
    registry: &'a Registry,
    profile: Option<String>,
    project: Option<Project>,
    ceiling: Policy,
    resolved: Resolved,
    owner: u64,
    connections: BTreeMap<String, Connection>,
    abandoned: BTreeMap<String, Vec<crate::Abandoned>>,
    parent: TurnContext,
}
impl<'a> Manager<'a> {
    pub fn new(
        registry: &'a Registry,
        profile: Option<String>,
        project: Option<Project>,
        ceiling: Policy,
        parent: &TurnContext,
    ) -> Result<Self, Error> {
        parent.remaining()?;
        let _lock = registry.lock()?;
        let resolved = registry.resolve(profile.as_deref(), project.as_ref(), &ceiling)?;
        Ok(Self {
            registry,
            profile,
            project,
            ceiling,
            resolved,
            owner: OWNERS.fetch_add(1, Ordering::Relaxed),
            connections: BTreeMap::new(),
            abandoned: BTreeMap::new(),
            parent: parent.clone(),
        })
    }
    fn retire_connections(&mut self) {
        // Cleanup is allowed even after project trust or parent context ends.
        if let Ok(cleanup) =
            TurnContext::new(Duration::from_secs(2), namzu_core::Cancellation::default())
        {
            let _ = self.close(&cleanup);
        } else {
            self.connections.clear();
        }
    }
    fn refresh(&mut self) -> Result<(), Error> {
        let resolved = match self.registry.resolve(
            self.profile.as_deref(),
            self.project.as_ref(),
            &self.ceiling,
        ) {
            Ok(r) => r,
            Err(e) => {
                self.retire_connections();
                return Err(e);
            }
        };
        if resolved.fingerprint != self.resolved.fingerprint {
            self.retire_connections();
            self.resolved = resolved;
        }
        Ok(())
    }
    pub fn prepare_connect(&mut self, server: &str) -> Result<Plan, Error> {
        self.prepare(server, None, None)
    }
    pub fn prepare_call(
        &mut self,
        server: &str,
        method: &str,
        params: Option<Map<String, Value>>,
    ) -> Result<Plan, Error> {
        // Strict bounds and method allowlist before admission or any I/O.
        if ![
            "ping",
            "tools/list",
            "tools/call",
            "resources/list",
            "resources/templates/list",
            "resources/read",
            "resources/subscribe",
            "resources/unsubscribe",
            "prompts/list",
            "prompts/get",
            "logging/setLevel",
            "completion/complete",
        ]
        .contains(&method)
            || params.as_ref().is_some_and(|p| p.contains_key("task"))
        {
            return Err(Error::Denied);
        }
        let message = Message::Request {
            id: Id::String("admission".into()),
            method: method.into(),
            params,
        };
        crate::wire::encode(&message, &Limits::default())?;
        let Message::Request { params, .. } = message else {
            unreachable!()
        };
        self.prepare(server, Some(method.into()), params)
    }
    fn prepare(
        &mut self,
        server: &str,
        method: Option<String>,
        params: Option<Map<String, Value>>,
    ) -> Result<Plan, Error> {
        self.parent.remaining()?;
        let _lock = self.registry.lock()?;
        self.refresh()?;
        let entry = self.resolved.servers.get(server).ok_or(Error::NotFound)?;
        let target = target(method.as_deref(), params.as_ref())?;
        Ok(Plan {
            owner: self.owner,
            fingerprint: self.resolved.fingerprint.clone(),
            server: server.into(),
            method,
            params,
            decision: entry.decide(&target),
        })
    }
    pub fn execute(&mut self, permit: Permit, context: &TurnContext) -> Result<Outcome, Error> {
        self.parent.remaining()?;
        context.remaining()?;
        if !context.is_bounded_by(&self.parent) {
            return Err(Error::Configuration);
        }
        let plan = permit.0;
        if plan.owner != self.owner {
            return Err(Error::Stale);
        }
        let _lock = self.registry.lock()?;
        self.refresh()?;
        if plan.fingerprint != self.resolved.fingerprint {
            return Err(Error::Stale);
        }
        let entry = self
            .resolved
            .servers
            .get(&plan.server)
            .ok_or(Error::NotFound)?;
        if entry.decide(&target(plan.method.as_deref(), plan.params.as_ref())?) == Decision::Deny {
            return Err(Error::Denied);
        }
        let context = context.child(Duration::from_secs(entry.server.timeout_secs))?;
        let server = entry.server.clone();
        if let Some(method) = plan.method {
            let connection = self
                .connections
                .get_mut(&plan.server)
                .ok_or(Error::NotConnected)?;
            let result = match connection {
                #[cfg(unix)]
                Connection::Stdio(client) => client
                    .call(&method, plan.params, &context)
                    .map(Outcome::Reply)
                    .map_err(Error::from),
                Connection::Http(client) => match client.call(&method, plan.params, &context) {
                    Ok(Incoming::Reply { result, .. }) => Ok(Outcome::Reply(result)),
                    Ok(_) => Err(Error::Configuration),
                    Err(e) => Err(e.into()),
                },
            };
            if result.is_err()
                && let Some(connection) = self.connections.get_mut(&plan.server)
            {
                let abandoned = match connection {
                    #[cfg(unix)]
                    Connection::Stdio(client) => client.close().abandoned,
                    Connection::Http(client) => client.abandoned().to_vec(),
                };
                if !abandoned.is_empty() {
                    self.abandoned.insert(plan.server.clone(), abandoned);
                }
            }
            result
        } else {
            if self.abandoned.contains_key(&plan.server) {
                return Err(Error::Uncertain);
            }
            if self.connections.len() >= 16 {
                return Err(Error::Configuration);
            }
            if self.connections.contains_key(&plan.server) {
                return Err(Error::Connected);
            }
            let info = Implementation::new("namzu-rs", "0.1.0");
            let connection = match server.transport {
                Transport::Http { url, bearer_env } => {
                    let mut spec = crate::http::Spec::new(url);
                    if let Some(name) = bearer_env {
                        spec.bearer = Some(std::env::var(name).map_err(|_| Error::Configuration)?);
                    }
                    Connection::Http(Box::new(crate::http::Client::connect(
                        spec, info, &context,
                    )?))
                }
                Transport::Stdio {
                    program,
                    args,
                    cwd,
                    env,
                } => {
                    #[cfg(unix)]
                    {
                        let mut spec = crate::stdio::Spec::new(program, cwd);
                        spec.args = args;
                        // Read only explicitly selected variables after admission.
                        for (key, selector) in env {
                            spec.env.insert(
                                key,
                                std::env::var(selector).map_err(|_| Error::Configuration)?,
                            );
                        }
                        Connection::Stdio(Box::new(crate::stdio::Client::connect_in_context(
                            spec,
                            info,
                            Limits::default(),
                            &self.parent,
                            &context,
                        )?))
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = (program, args, cwd, env, info);
                        return Err(Error::Unsupported);
                    }
                }
            };
            self.connections.insert(plan.server, connection);
            Ok(Outcome::Connected)
        }
    }
    /// In-memory evidence only; no automatic replay/reconnect or durable recovery.
    pub fn abandoned(&self) -> &BTreeMap<String, Vec<crate::Abandoned>> {
        &self.abandoned
    }
    pub fn statuses(&self) -> Vec<Status> {
        self.connections
            .iter()
            .map(|(name, connection)| Status {
                server: name.clone(),
                phase: match connection {
                    #[cfg(unix)]
                    Connection::Stdio(client) => {
                        if client.phase() == crate::Phase::Ready {
                            "connected"
                        } else {
                            "closed"
                        }
                    }
                    Connection::Http(client) => {
                        if client.phase() == crate::Phase::Ready {
                            "connected"
                        } else {
                            "closed"
                        }
                    }
                },
            })
            .collect()
    }
    /// Explicit cleanup, independent of an expired task/permission snapshot.
    pub fn close(&mut self, context: &TurnContext) -> Result<(), Error> {
        let mut error = None;
        for (_, connection) in std::mem::take(&mut self.connections) {
            match connection {
                #[cfg(unix)]
                Connection::Stdio(mut client) => {
                    client.close();
                }
                Connection::Http(mut client) => {
                    if let Err(e) = client.close(context) {
                        error.get_or_insert(Error::Http(e));
                    }
                }
            }
        }
        match error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}
fn target(method: Option<&str>, params: Option<&Map<String, Value>>) -> Result<String, Error> {
    let Some(method) = method else {
        return Ok("connect".into());
    };
    let field = match method {
        "tools/call" | "prompts/get" => Some("name"),
        "resources/read" | "resources/subscribe" | "resources/unsubscribe" => Some("uri"),
        _ => None,
    };
    if let Some(field) = field {
        let name = params
            .and_then(|p| p.get(field))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= 4096 && !s.chars().any(char::is_control))
            .ok_or(Error::Configuration)?;
        Ok(format!("{method}:{name}"))
    } else {
        Ok(method.into())
    }
}
