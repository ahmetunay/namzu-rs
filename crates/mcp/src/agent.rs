//! Explicit core Tools adapter. No default servers, hidden approval or URI fetch.
use crate::{
    config::{Decision, Error},
    discovery::{Item, Kind, prompt_result, read_result, tool_result},
    manager::{Manager, Outcome, Permit, Plan},
    schema::Compiled,
};
use namzu_core::{
    AgentError, Event, Message, Observer, Stats, ToolCall, ToolDefinition, ToolResult, Tools,
    TurnContext,
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

pub fn alias(server: &str, kind: Kind, name: &str) -> String {
    let hash = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(server, kind, name)).expect("string tuple"))
    );
    let slug = |s: &str| {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(12)
            .collect::<String>()
    };
    format!("mcp_{}_{}_{hash}", slug(server), slug(name))[..]
        .chars()
        .take(63)
        .collect()
}
#[derive(Clone, Default)]
pub struct Approvals(Rc<RefCell<BTreeSet<String>>>);
fn signature(call: &ToolCall) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(call).expect("typed tool call"))
    )
}
impl Approvals {
    /// Called only by the host after a real permission decision. Single use.
    pub fn grant(&self, call: &ToolCall) -> Result<(), AgentError> {
        let mut grants = self
            .0
            .try_borrow_mut()
            .map_err(|_| AgentError::State("MCP approval busy"))?;
        if grants.len() >= 128 {
            return Err(AgentError::ToolLimit);
        }
        grants.insert(signature(call));
        Ok(())
    }
    fn take(&self, call: &ToolCall) -> bool {
        self.0.borrow_mut().remove(&signature(call))
    }
}
pub struct ApprovedObserver<'a, O: Observer> {
    pub inner: &'a mut O,
    pub approvals: Approvals,
    pub explicit: bool,
}
impl<O: Observer> Observer for ApprovedObserver<'_, O> {
    fn supports_approval(&self) -> bool {
        self.inner.supports_approval()
    }
    fn approve(&mut self, call: &ToolCall, ctx: &TurnContext) -> Result<bool, AgentError> {
        let approved = self.inner.approve(call, ctx)?;
        if approved && self.explicit && call.name.starts_with("mcp_") {
            self.approvals.grant(call)?;
        }
        Ok(approved)
    }
    fn event(&mut self, event: Event) -> Result<(), AgentError> {
        self.inner.event(event)
    }
    fn checkpoint(
        &mut self,
        m: &[Message],
        s: &Stats,
        p: Option<&ToolCall>,
    ) -> Result<(), AgentError> {
        self.inner.checkpoint(m, s, p)
    }
}
struct Binding {
    server: String,
    name: String,
    kind: Kind,
    input: Compiled,
    output: Option<Compiled>,
    definition: ToolDefinition,
    revision: u64,
}
pub struct AgentTools<'a, L: Tools> {
    local: L,
    manager: Manager<'a>,
    bindings: BTreeMap<String, Binding>,
    approvals: Approvals,
}
impl<'a, L: Tools> AgentTools<'a, L> {
    pub fn new(local: L, manager: Manager<'a>, approvals: Approvals) -> Self {
        Self {
            local,
            manager,
            bindings: BTreeMap::new(),
            approvals,
        }
    }
    pub fn manager_mut(&mut self) -> &mut Manager<'a> {
        &mut self.manager
    }
    /// Explicitly rebuild model bindings after a host reconnect. Never starts
    /// a transport, replays a call or retains old per-call approvals.
    pub fn rebind(
        &mut self,
        servers: &[String],
        allow: Option<&BTreeSet<String>>,
        deny: &BTreeSet<String>,
        ctx: &TurnContext,
        admit: impl FnMut(Plan) -> Result<Permit, Error>,
    ) -> Result<(), Error> {
        self.bindings.clear();
        self.approvals
            .0
            .try_borrow_mut()
            .map_err(|_| AgentError::State("MCP approval busy"))?
            .clear();
        let result = self.load_inner(servers, allow, deny, ctx, admit, false);
        if result.is_err() {
            self.bindings.clear();
        }
        result
    }
    /// Bootstrap explicit selected servers; host reviews connect and each page.
    pub fn load(
        &mut self,
        servers: &[String],
        allow: Option<&BTreeSet<String>>,
        deny: &BTreeSet<String>,
        ctx: &TurnContext,
        admit: impl FnMut(Plan) -> Result<Permit, Error>,
    ) -> Result<(), Error> {
        self.load_inner(servers, allow, deny, ctx, admit, true)
    }
    fn load_inner(
        &mut self,
        servers: &[String],
        allow: Option<&BTreeSet<String>>,
        deny: &BTreeSet<String>,
        ctx: &TurnContext,
        mut admit: impl FnMut(Plan) -> Result<Permit, Error>,
        connect: bool,
    ) -> Result<(), Error> {
        if servers.len() > 8 {
            return Err(Error::Configuration);
        }
        let mut seen = BTreeSet::new();
        for server in servers {
            if !seen.insert(server) {
                return Err(Error::Configuration);
            }
            if connect {
                let permission = admit(self.manager.prepare_connect(server)?)?;
                self.manager.execute(permission, ctx)?;
            }
            for kind in [Kind::Tools, Kind::Resources, Kind::Prompts] {
                // Absent/denied catalogues are not silently queried. Missing
                // capability is distinguished from network/protocol failure.
                if self.manager.decision(server, kind.method())? == Decision::Deny {
                    continue;
                }
                let catalog = match self.manager.discover(server, kind, ctx, &mut admit) {
                    Err(Error::Protocol(crate::Error::Capability)) => continue,
                    other => other?,
                };
                let revision = catalog.revision;
                for item in catalog.items {
                    let (name, parameters, description, output, target) = match item {
                        Item::Tool(tool) => (
                            tool.name.clone(),
                            Value::Object(tool.input_schema),
                            tool.description.unwrap_or_default(),
                            tool.output_schema,
                            format!("tools/call:{}", tool.name),
                        ),
                        Item::Resource(resource) => (
                            resource.uri.clone(),
                            json!({"type":"object","properties":{},"additionalProperties":false}),
                            resource.description.unwrap_or_default(),
                            None,
                            format!("resources/read:{}", resource.uri),
                        ),
                        Item::Prompt(prompt) => {
                            let mut properties = Map::new();
                            let mut required = vec![];
                            for argument in prompt.arguments {
                                properties.insert(argument.name.clone(), json!({"type":"string"}));
                                if argument.required {
                                    required.push(argument.name);
                                }
                            }
                            (
                                prompt.name.clone(),
                                json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
                                prompt.description.unwrap_or_default(),
                                None,
                                format!("prompts/get:{}", prompt.name),
                            )
                        }
                        Item::Template(_) => continue,
                    };
                    if self.manager.decision(server, &target)? == Decision::Deny {
                        continue;
                    }
                    let key = alias(server, kind, &name);
                    if deny.contains(&key) || allow.is_some_and(|allow| !allow.contains(&key)) {
                        continue;
                    }
                    if self.bindings.len() + self.local.definitions().len() >= 128
                        || self.bindings.contains_key(&key)
                        || self.local.definitions().iter().any(|d| d.name == key)
                    {
                        return Err(Error::Configuration);
                    }
                    let input = Compiled::new(parameters.as_object().ok_or(Error::Configuration)?)?;
                    let output = output.as_ref().map(Compiled::new).transpose()?;
                    self.bindings.insert(
                        key.clone(),
                        Binding {
                            server: server.clone(),
                            name,
                            kind,
                            input,
                            output,
                            revision,
                            definition: ToolDefinition {
                                name: key,
                                description: format!("MCP {server}: {description}"),
                                parameters,
                            },
                        },
                    );
                }
            }
        }
        let bytes = serde_json::to_vec(&self.definitions())
            .map_err(|_| Error::Configuration)?
            .len();
        if bytes > 256 * 1024 {
            return Err(Error::Configuration);
        }
        Ok(())
    }
    pub fn close(&mut self, ctx: &TurnContext) -> Result<(), Error> {
        self.manager.close(ctx)
    }
    pub fn into_local(self) -> L {
        self.local
    }
}
fn fatal(error: Error) -> AgentError {
    match error {
        Error::State(e) => e,
        #[cfg(unix)]
        Error::Stdio(crate::stdio::Error::Context(e)) => e,
        Error::Http(crate::http::Error::Context(e)) => e,
        _ => AgentError::Protocol("MCP operation failed; inspect session before retry"),
    }
}
impl<L: Tools> Tools for AgentTools<'_, L> {
    fn definitions(&self) -> Vec<ToolDefinition> {
        let mut definitions = self.local.definitions();
        definitions.extend(self.bindings.values().map(|b| b.definition.clone()));
        definitions
    }
    fn validate(&mut self, call: &ToolCall, ctx: &TurnContext) -> Result<(), AgentError> {
        if let Some(binding) = self.bindings.get(&call.name) {
            let catalog = self
                .manager
                .catalog(&binding.server, binding.kind)
                .map_err(fatal)?;
            if catalog.is_none_or(|c| c.revision != binding.revision) {
                return Err(AgentError::Protocol("MCP binding is stale"));
            }
            binding.input.validate(&call.arguments, ctx)
        } else {
            self.local.validate(call, ctx)
        }
    }
    fn call(&mut self, call: &ToolCall, ctx: &TurnContext) -> Result<ToolResult, AgentError> {
        self.validate(call, ctx)?;
        let Some(binding) = self.bindings.get(&call.name) else {
            return self.local.call(call, ctx);
        };
        let args = call
            .arguments
            .as_object()
            .ok_or(AgentError::Protocol("MCP arguments required"))?
            .clone();
        let plan = match binding.kind {
            Kind::Tools => self
                .manager
                .prepare_tool_call(&binding.server, &binding.name, args),
            Kind::Resources => self
                .manager
                .prepare_resource_read(&binding.server, &binding.name),
            Kind::Prompts => self
                .manager
                .prepare_prompt_get(&binding.server, &binding.name, args),
            Kind::Templates => {
                return Err(AgentError::Protocol("template requires explicit expansion"));
            }
        }
        .map_err(fatal)?;
        let granted = self.approvals.take(call);
        let permit = match if granted {
            plan.approve()
        } else {
            plan.permit()
        } {
            Ok(p) => p,
            Err(Error::Denied | Error::ApprovalRequired) => {
                return Ok(ToolResult::error("MCP permission denied"));
            }
            Err(e) => return Err(fatal(e)),
        };
        let result = match self.manager.execute(permit, ctx).map_err(fatal)? {
            Outcome::Reply(Ok(result)) => result,
            Outcome::Reply(Err(_)) => {
                return Ok(ToolResult::error("MCP server rejected the request"));
            }
            _ => return Err(AgentError::Protocol("unexpected MCP outcome")),
        };
        let (value, is_error) = match binding.kind {
            Kind::Tools => {
                let result = tool_result(result)
                    .map_err(|_| AgentError::Protocol("invalid MCP tool result"))?;
                if !result.is_error
                    && let Some(schema) = &binding.output
                {
                    let value = result
                        .structured_content
                        .as_ref()
                        .ok_or(AgentError::Protocol("MCP structured output missing"))?;
                    schema.validate(&Value::Object(value.clone()), ctx)?;
                }
                let error = result.is_error;
                (serde_json::to_value(result), error)
            }
            Kind::Resources => (
                serde_json::to_value(
                    read_result(result)
                        .map_err(|_| AgentError::Protocol("invalid MCP resource result"))?,
                ),
                false,
            ),
            Kind::Prompts => (
                serde_json::to_value(
                    prompt_result(result)
                        .map_err(|_| AgentError::Protocol("invalid MCP prompt result"))?,
                ),
                false,
            ),
            Kind::Templates => unreachable!(),
        };
        let content = serde_json::to_string(
            &value.map_err(|_| AgentError::Protocol("cannot encode MCP content"))?,
        )
        .map_err(|_| AgentError::Protocol("cannot encode MCP content"))?;
        if content.len() > 1024 * 1024 {
            return Err(AgentError::Protocol("MCP content exceeds limit"));
        }
        Ok(ToolResult { content, is_error })
    }
}
