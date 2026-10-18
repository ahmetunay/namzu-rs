//! In-process, thread-affine server ownership behind one-operation admission.
use crate::{
    Id, Incoming, Limits, Message,
    config::{Decision, Error, Policy, Project, Registry, Resolved, Transport},
    discovery::{Catalog, Kind, Notification, Pager},
    types::Implementation,
};
use namzu_core::TurnContext;
use serde::Serialize;
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
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
    catalog_revision: Option<(Kind, u64)>,
    generation: u64,
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
    Events { supported: bool },
}
#[derive(Serialize)]
pub struct Status {
    pub server: String,
    pub phase: &'static str,
    pub generation: u64,
    pub subscriptions: usize,
}
enum Connection {
    #[cfg(unix)]
    Stdio(Box<crate::stdio::Client>),
    Http(Box<crate::http::Client>),
}
/// Host supplied launcher for an owned guest. A guest manager rejects HTTP;
/// there is no implicit host execution/network fallback.
#[cfg(unix)]
pub trait GuestStdio {
    fn recovery_scope(&self) -> String {
        "custom-guest".into()
    }
    fn check(&mut self, context: &TurnContext) -> Result<(), Error> {
        context.remaining()?;
        Ok(())
    }
    fn launch(
        &mut self,
        spec: crate::stdio::Spec,
        context: &TurnContext,
    ) -> Result<crate::stdio::Spec, Error>;
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
    catalogs: BTreeMap<(String, Kind), Catalog>,
    catalog_sizes: BTreeMap<(String, Kind), usize>,
    catalog_bytes: usize,
    revision: u64,
    epochs: BTreeMap<(String, Kind), u64>,
    notices: VecDeque<(Notification, usize)>,
    notice_bytes: usize,
    generations: BTreeMap<String, u64>,
    subscriptions: BTreeMap<String, BTreeSet<String>>,
    updated: BTreeMap<String, BTreeSet<String>>,
    recovery_owner: Option<crate::recovery::Owner>,
    #[cfg(unix)]
    guest: Option<Box<dyn GuestStdio + 'a>>,
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
            catalogs: BTreeMap::new(),
            catalog_sizes: BTreeMap::new(),
            catalog_bytes: 0,
            revision: 1,
            epochs: BTreeMap::new(),
            notices: VecDeque::new(),
            notice_bytes: 0,
            generations: BTreeMap::new(),
            subscriptions: BTreeMap::new(),
            updated: BTreeMap::new(),
            recovery_owner: None,
            #[cfg(unix)]
            guest: None,
        })
    }
    #[cfg(unix)]
    pub fn with_guest(mut self, launcher: Box<dyn GuestStdio + 'a>) -> Self {
        // Switching execution location invalidates all host connections,
        // discovered bindings and previously issued permits.
        self.retire_connections();
        self.owner = OWNERS.fetch_add(1, Ordering::Relaxed);
        self.guest = Some(launcher);
        self
    }
    pub fn identity(&self) -> &str {
        &self.resolved.fingerprint
    }
    pub fn decision(&mut self, server: &str, target: &str) -> Result<Decision, Error> {
        let _lock = self.registry.lock()?;
        self.refresh()?;
        Ok(self
            .resolved
            .servers
            .get(server)
            .ok_or(Error::NotFound)?
            .decide(target))
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
    /// Explicit fresh transport; never replays calls or restores subscriptions.
    pub fn prepare_reconnect(&mut self, server: &str) -> Result<Plan, Error> {
        let mut plan = self.prepare(server, Some("connection/reconnect".into()), None)?;
        let entry = self.resolved.servers.get(server).ok_or(Error::NotFound)?;
        plan.decision = plan.decision.max(entry.decide("connect"));
        Ok(plan)
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
            catalog_revision: None,
            generation: *self.generations.get(server).unwrap_or(&0),
        })
    }
    pub fn execute(&mut self, permit: Permit, context: &TurnContext) -> Result<Outcome, Error> {
        self.parent.remaining()?;
        context.remaining()?;
        if !context.is_bounded_by(&self.parent) {
            return Err(Error::Configuration);
        }
        let mut plan = permit.0;
        if plan.owner != self.owner
            || plan.generation != *self.generations.get(&plan.server).unwrap_or(&0)
        {
            return Err(Error::Stale);
        }
        let _lock = self.registry.lock()?;
        self.refresh()?;
        if plan.fingerprint != self.resolved.fingerprint {
            return Err(Error::Stale);
        }
        if let Some((kind, revision)) = plan.catalog_revision
            && self
                .catalogs
                .get(&(plan.server.clone(), kind))
                .is_none_or(|c| c.revision != revision)
        {
            return Err(crate::discovery::Error::Stale.into());
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
        #[cfg(unix)]
        if let Some(guest) = &mut self.guest {
            guest.check(&context)?;
        }
        let target_key = self.recovery_target(&plan.server, &server)?;
        if plan.method.as_deref() == Some("connection/reconnect") {
            if entry.decide("connect") == Decision::Deny {
                return Err(Error::Denied);
            }
            self.registry
                .recovery()?
                .check(&target_key, self.recovery_owner.as_ref())?;
            self.retire_server(&plan.server)?;
            self.abandoned.remove(&plan.server);
            plan.method = None;
        }
        if let Some(method) = plan.method {
            self.validate_subscription(&plan.server, &method, plan.params.as_ref())?;
            if !self.connections.contains_key(&plan.server) {
                return Err(Error::NotConnected);
            }
            let ticket = if method == "tools/call" {
                let store = self.registry.recovery()?;
                if self.recovery_owner.is_none() {
                    self.recovery_owner = Some(store.owner()?);
                }
                Some(store.begin(
                    self.recovery_owner.as_ref().ok_or(Error::Configuration)?,
                    target_key,
                    plan.server.clone(),
                    &plan.params,
                )?)
            } else {
                None
            };
            let subscription_uri = plan
                .params
                .as_ref()
                .and_then(|p| p.get("uri"))
                .and_then(Value::as_str)
                .map(String::from);
            let connection = self
                .connections
                .get_mut(&plan.server)
                .ok_or(Error::NotConnected)?;
            let mut notices = vec![];
            let mut bytes = 0usize;
            let result = match connection {
                #[cfg(unix)]
                Connection::Stdio(client) => {
                    if method == "events/listen" {
                        match client.next(&context) {
                            Ok(event @ Incoming::Notification { .. }) => {
                                notices.push(event);
                                Ok(Outcome::Events { supported: true })
                            }
                            Ok(_) => Err(Error::Configuration),
                            Err(e) => Err(e.into()),
                        }
                    } else {
                        client
                            .call_observed(&method, plan.params, &context, |event| {
                                let size = notification_size(&event)
                                    .map_err(|_| crate::stdio::Error::Backpressure)?;
                                if notices.len() >= 64
                                    || bytes.saturating_add(size) > 2 * 1024 * 1024
                                {
                                    return Err(crate::stdio::Error::Backpressure);
                                }
                                bytes += size;
                                notices.push(event);
                                Ok(())
                            })
                            .map(Outcome::Reply)
                            .map_err(Error::from)
                    }
                }
                Connection::Http(client) => {
                    let result = if method == "events/listen" {
                        match client.listen(&context) {
                            Ok(Some(event)) => {
                                notices.push(event);
                                Ok(Outcome::Events { supported: true })
                            }
                            Ok(None) => Ok(Outcome::Events { supported: false }),
                            Err(e) => Err(e.into()),
                        }
                    } else {
                        match client.call(&method, plan.params, &context) {
                            Ok(Incoming::Reply { result, .. }) => Ok(Outcome::Reply(result)),
                            Ok(_) => Err(Error::Configuration),
                            Err(e) => Err(e.into()),
                        }
                    };
                    while let Some(event) = client.pop_event() {
                        notices.push(event);
                    }
                    result
                }
            };
            if let Ok(Outcome::Reply(Ok(_))) = &result
                && let Some(uri) = subscription_uri
            {
                if method == "resources/subscribe" {
                    self.subscriptions
                        .entry(plan.server.clone())
                        .or_default()
                        .insert(uri);
                } else if method == "resources/unsubscribe" {
                    self.subscriptions
                        .entry(plan.server.clone())
                        .or_default()
                        .remove(&uri);
                    self.updated
                        .entry(plan.server.clone())
                        .or_default()
                        .remove(&uri);
                }
            }
            let observed = self.observe(&plan.server, notices);
            if observed.is_err()
                && let Some(connection) = self.connections.get_mut(&plan.server)
            {
                match connection {
                    #[cfg(unix)]
                    Connection::Stdio(client) => {
                        client.close();
                    }
                    Connection::Http(client) => client.fence_generation(),
                }
            }
            let result = result.and_then(|outcome| {
                observed?;
                Ok(outcome)
            });
            if let Some(ticket) = ticket
                && let Err(error) = self.registry.recovery()?.finish(ticket, result.is_ok())
            {
                let _ = self.retire_server(&plan.server);
                return Err(error);
            }
            if result.is_err() {
                self.fence_bindings(&plan.server);
                self.invalidate_server(&plan.server);
                self.revision += 1;
            }
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
            self.registry
                .recovery()?
                .check(&target_key, self.recovery_owner.as_ref())?;
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
                    #[cfg(unix)]
                    if self.guest.is_some() {
                        return Err(Error::Unsupported);
                    }
                    let mut spec = crate::http::Spec::new(url);
                    if let Some(settings) = server.oauth {
                        let target = crate::oauth::Target::new(spec.endpoint.clone(), settings)?;
                        spec.bearer = Some(
                            crate::oauth::Vault::new(self.registry.oauth_store()?)
                                .access_token(&target, &context)?,
                        );
                    }
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
                        if let Some(guest) = &mut self.guest {
                            spec = guest.launch(spec, &context)?;
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
            self.fence_bindings(&plan.server);
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
                generation: *self.generations.get(name).unwrap_or(&0),
                subscriptions: self.subscriptions.get(name).map_or(0, BTreeSet::len),
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
        self.owner = OWNERS.fetch_add(1, Ordering::Relaxed);
        self.subscriptions.clear();
        self.updated.clear();
        for generation in self.generations.values_mut() {
            *generation += 1;
        }
        self.catalogs.clear();
        self.catalog_sizes.clear();
        self.catalog_bytes = 0;
        self.epochs.clear();
        self.revision += 1;
        self.notices.clear();
        self.notice_bytes = 0;
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

impl Plan {
    pub fn server(&self) -> &str {
        &self.server
    }
    pub fn method(&self) -> Option<&str> {
        self.method.as_deref()
    }
    pub fn params(&self) -> Option<&Map<String, Value>> {
        self.params.as_ref()
    }
}
fn notification_size(event: &Incoming) -> Result<usize, Error> {
    if let Incoming::Notification { method, params } = event {
        let message = Message::Notification {
            method: method.clone(),
            params: params.clone(),
        };
        Ok(crate::wire::encode(&message, &Limits::default())?.len())
    } else {
        Err(Error::Configuration)
    }
}
impl Manager<'_> {
    fn invalidate_catalog(&mut self, key: &(String, Kind)) {
        self.catalogs.remove(key);
        if let Some(size) = self.catalog_sizes.remove(key) {
            self.catalog_bytes -= size;
        }
    }
    fn invalidate_server(&mut self, server: &str) {
        for kind in [Kind::Tools, Kind::Resources, Kind::Prompts, Kind::Templates] {
            self.invalidate_catalog(&(server.into(), kind));
        }
    }
    fn capabilities(&self, server: &str) -> Result<crate::types::ServerCapabilities, Error> {
        let connection = self.connections.get(server).ok_or(Error::NotConnected)?;
        let (phase, server) = match connection {
            #[cfg(unix)]
            Connection::Stdio(client) => (client.phase(), client.server()),
            Connection::Http(client) => (client.phase(), client.server()),
        };
        if phase != crate::Phase::Ready {
            return Err(Error::NotConnected);
        }
        Ok(server.ok_or(Error::NotConnected)?.capabilities.clone())
    }
    fn observe(&mut self, server: &str, events: Vec<Incoming>) -> Result<(), Error> {
        // Capabilities can still be read on a transport fenced after receiving data.
        let caps = self
            .connections
            .get(server)
            .and_then(|connection| match connection {
                #[cfg(unix)]
                Connection::Stdio(client) => client.server(),
                Connection::Http(client) => client.server(),
            })
            .map(|s| s.capabilities.clone())
            .unwrap_or_default();
        for event in events {
            let size = notification_size(&event)?;
            if self.notices.len() >= 64 || self.notice_bytes.saturating_add(size) > 2 * 1024 * 1024
            {
                return Err(crate::discovery::Error::Limit.into());
            }
            let Incoming::Notification { method, params } = event else {
                return Err(Error::Configuration);
            };
            let kind = match method.as_str() {
                "notifications/tools/list_changed"
                    if caps
                        .tools
                        .as_ref()
                        .is_some_and(|c| c.list_changed == Some(true)) =>
                {
                    Some(Kind::Tools)
                }
                "notifications/prompts/list_changed"
                    if caps
                        .prompts
                        .as_ref()
                        .is_some_and(|c| c.list_changed == Some(true)) =>
                {
                    Some(Kind::Prompts)
                }
                "notifications/resources/list_changed"
                    if caps
                        .resources
                        .as_ref()
                        .is_some_and(|c| c.list_changed == Some(true)) =>
                {
                    Some(Kind::Resources)
                }
                _ => None,
            };
            if let Some(kind) = kind {
                let kinds = if kind == Kind::Resources {
                    vec![Kind::Resources, Kind::Templates]
                } else {
                    vec![kind]
                };
                for kind in kinds {
                    self.invalidate_catalog(&(server.into(), kind));
                    *self.epochs.entry((server.into(), kind)).or_default() += 1;
                }
            }
            if method == "notifications/resources/updated" {
                let uri = params
                    .as_ref()
                    .and_then(|p| p.get("uri"))
                    .and_then(Value::as_str)
                    .filter(|s| {
                        !s.is_empty() && s.len() <= 4096 && !s.chars().any(char::is_control)
                    })
                    .ok_or(Error::Subscription)?;
                if self
                    .subscriptions
                    .get(server)
                    .is_some_and(|s| s.contains(uri))
                {
                    self.updated
                        .entry(server.into())
                        .or_default()
                        .insert(uri.into());
                }
            }
            // Unknown/progress/resource-updated notifications remain bounded inert
            // data. They never open a URI, execute a tool, or auto-refresh a list.
            self.notice_bytes += size;
            self.notices.push_back((
                Notification {
                    server: server.into(),
                    generation: *self.generations.get(server).unwrap_or(&0),
                    method,
                    params,
                },
                size,
            ));
        }
        Ok(())
    }
    pub fn pop_notification(&mut self) -> Option<Notification> {
        self.notices.pop_front().map(|(event, size)| {
            self.notice_bytes -= size;
            event
        })
    }
    /// Explicit optional event pump, admitted independently from RPC methods.
    /// Stdio waits for one notification; HTTP GET 405 returns supported=false.
    pub fn prepare_notifications(&mut self, server: &str) -> Result<Plan, Error> {
        self.prepare(server, Some("events/listen".into()), None)
    }
    pub fn catalog(&mut self, server: &str, kind: Kind) -> Result<Option<&Catalog>, Error> {
        self.parent.remaining()?;
        let _lock = self.registry.lock()?;
        self.refresh()?;
        Ok(self.catalogs.get(&(server.into(), kind)))
    }
    /// Every page needs its own grant. No implicit retry, startup or partial cache.
    /// Approval callback is host code and must review Plan before approving it.
    pub fn discover(
        &mut self,
        server: &str,
        kind: Kind,
        context: &TurnContext,
        mut admit: impl FnMut(Plan) -> Result<Permit, Error>,
    ) -> Result<Catalog, Error> {
        self.parent.remaining()?;
        context.remaining()?;
        if !context.is_bounded_by(&self.parent) {
            return Err(Error::Configuration);
        }
        {
            let _lock = self.registry.lock()?;
            self.refresh()?;
        }
        if !kind.supported(&self.capabilities(server)?) {
            return Err(crate::Error::Capability.into());
        }
        let key = (server.to_owned(), kind);
        self.invalidate_catalog(&key);
        let epoch = *self.epochs.get(&key).unwrap_or(&0);
        let fingerprint = self.resolved.fingerprint.clone();
        let mut pager = Pager::new(kind);
        loop {
            context.remaining()?;
            let params = pager.request_params()?;
            let plan = self.prepare_call(server, kind.method(), params.clone())?;
            let permit = admit(plan)?;
            if permit.0.server != server
                || permit.0.method.as_deref() != Some(kind.method())
                || permit.0.params != params
            {
                return Err(Error::Stale);
            }
            let outcome = self.execute(permit, context)?;
            if self.resolved.fingerprint != fingerprint
                || *self.epochs.get(&key).unwrap_or(&0) != epoch
            {
                return Err(crate::discovery::Error::Stale.into());
            }
            let result = match outcome {
                Outcome::Reply(Ok(result)) => result,
                Outcome::Reply(Err(_)) => return Err(crate::discovery::Error::Rpc.into()),
                _ => return Err(Error::Configuration),
            };
            if pager.push(result)? {
                break;
            }
        }
        // Local validation may consume time: recheck context/trust before publish.
        context.remaining()?;
        let _lock = self.registry.lock()?;
        self.refresh()?;
        if self.resolved.fingerprint != fingerprint || *self.epochs.get(&key).unwrap_or(&0) != epoch
        {
            return Err(crate::discovery::Error::Stale.into());
        }
        self.revision += 1;
        let catalog = pager.finish(server.into(), self.revision)?;
        let size = serde_json::to_vec(&catalog)
            .map_err(|_| crate::discovery::Error::Payload)?
            .len();
        if self.catalog_bytes.saturating_add(size) > 8 * 1024 * 1024 {
            return Err(crate::discovery::Error::Limit.into());
        }
        context.remaining()?;
        self.catalog_bytes += size;
        self.catalog_sizes.insert(key.clone(), size);
        self.catalogs.insert(key, catalog.clone());
        Ok(catalog)
    }
    /// Catalogs supply names, never permissions. This still returns an ordinary
    /// one-operation plan. JSON Schema instance evaluation belongs to 3.6.
    pub fn prepare_tool_call(
        &mut self,
        server: &str,
        name: &str,
        arguments: Map<String, Value>,
    ) -> Result<Plan, Error> {
        self.prepare_catalog_call(server, Kind::Tools, name, Some(arguments))
    }
    pub fn prepare_resource_read(&mut self, server: &str, uri: &str) -> Result<Plan, Error> {
        self.prepare_catalog_call(server, Kind::Resources, uri, None)
    }
    pub fn prepare_prompt_get(
        &mut self,
        server: &str,
        name: &str,
        arguments: Map<String, Value>,
    ) -> Result<Plan, Error> {
        self.prepare_catalog_call(server, Kind::Prompts, name, Some(arguments))
    }
    fn prepare_catalog_call(
        &mut self,
        server: &str,
        kind: Kind,
        name: &str,
        arguments: Option<Map<String, Value>>,
    ) -> Result<Plan, Error> {
        let (method, field) = match kind {
            Kind::Tools => ("tools/call", "name"),
            Kind::Resources => ("resources/read", "uri"),
            Kind::Prompts => ("prompts/get", "name"),
            Kind::Templates => return Err(Error::Configuration),
        };
        let mut params = Map::from_iter([(field.into(), Value::String(name.into()))]);
        if let Some(arguments) = arguments {
            params.insert("arguments".into(), Value::Object(arguments));
        }
        let mut plan = self.prepare_call(server, method, Some(params))?;
        let catalog = self
            .catalogs
            .get(&(server.into(), kind))
            .ok_or(crate::discovery::Error::Stale)?;
        let item = catalog
            .items
            .iter()
            .find(|item| item.identity() == name)
            .ok_or(crate::discovery::Error::Stale)?;
        if let crate::discovery::Item::Prompt(prompt) = item {
            let arguments = plan
                .params
                .as_ref()
                .and_then(|p| p.get("arguments"))
                .and_then(Value::as_object)
                .ok_or(Error::Configuration)?;
            if arguments.iter().any(|(name, value)| {
                !value.is_string() || !prompt.arguments.iter().any(|a| &a.name == name)
            }) || prompt
                .arguments
                .iter()
                .any(|a| a.required && !arguments.contains_key(&a.name))
            {
                return Err(crate::discovery::Error::Payload.into());
            }
        }
        plan.catalog_revision = Some((kind, catalog.revision));
        Ok(plan)
    }
}

impl Manager<'_> {
    fn recovery_target(&self, name: &str, server: &crate::config::Server) -> Result<String, Error> {
        use sha2::{Digest, Sha256};
        let mut location = "host".to_string();
        #[cfg(unix)]
        if let Some(guest) = &self.guest {
            location = format!("guest:{}", guest.recovery_scope());
        }
        let bytes = serde_json::to_vec(&(name, &server.transport, &server.oauth, location))
            .map_err(|_| Error::Configuration)?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
    fn fence_bindings(&mut self, server: &str) {
        *self.generations.entry(server.into()).or_default() += 1;
        self.subscriptions.remove(server);
        self.updated.remove(server);
        self.invalidate_server(server);
        self.notices.retain(|(n, _)| n.server != server);
        self.notice_bytes = self.notices.iter().map(|(_, size)| size).sum();
    }
    fn retire_server(&mut self, server: &str) -> Result<(), Error> {
        self.fence_bindings(server);
        if let Some(connection) = self.connections.remove(server) {
            match connection {
                #[cfg(unix)]
                Connection::Stdio(mut c) => {
                    c.close();
                }
                Connection::Http(mut c) => {
                    let cleanup = TurnContext::new(
                        Duration::from_secs(2),
                        namzu_core::Cancellation::default(),
                    )?;
                    c.close(&cleanup)?;
                }
            }
        }
        Ok(())
    }
    fn validate_subscription(
        &self,
        server: &str,
        method: &str,
        params: Option<&Map<String, Value>>,
    ) -> Result<(), Error> {
        if !["resources/subscribe", "resources/unsubscribe"].contains(&method) {
            return Ok(());
        }
        if !self
            .capabilities(server)?
            .resources
            .is_some_and(|c| c.subscribe == Some(true))
        {
            return Err(crate::Error::Capability.into());
        }
        let p = params.ok_or(Error::Subscription)?;
        let uri = p
            .get("uri")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= 4096 && !s.chars().any(char::is_control))
            .ok_or(Error::Subscription)?;
        if p.keys().any(|key| key != "uri" && key != "_meta")
            || p.get("_meta").is_some_and(|v| !v.is_object())
        {
            return Err(Error::Subscription);
        }
        let subs = self.subscriptions.get(server);
        let exists = subs.is_some_and(|s| s.contains(uri));
        if method == "resources/subscribe" && (exists || subs.map_or(0, BTreeSet::len) >= 128)
            || method == "resources/unsubscribe" && !exists
        {
            return Err(Error::Subscription);
        }
        Ok(())
    }
    pub fn subscriptions(&mut self, server: &str) -> Result<Vec<String>, Error> {
        let _lock = self.registry.lock()?;
        self.refresh()?;
        if !self.resolved.servers.contains_key(server) {
            return Err(Error::NotFound);
        }
        Ok(self
            .subscriptions
            .get(server)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default())
    }
    pub fn take_resource_updates(&mut self, server: &str) -> Result<Vec<String>, Error> {
        let _lock = self.registry.lock()?;
        self.refresh()?;
        if !self.resolved.servers.contains_key(server) {
            return Err(Error::NotFound);
        }
        Ok(self
            .updated
            .remove(server)
            .unwrap_or_default()
            .into_iter()
            .collect())
    }
}
