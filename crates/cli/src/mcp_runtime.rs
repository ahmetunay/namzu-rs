use clap::Args;
use namzu_core::{AgentError, Cancellation, EventSink, ToolCall, Tools, TurnContext};
use namzu_mcp::{
    agent::{AgentTools, Approvals},
    config::{Decision, Error, Policy, Project, Registry},
    manager::{Manager, Permit, Plan},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::Duration,
};
#[derive(Clone, Default, Args, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Selection {
    /// Explicit MCP server selection; repeat for multiple servers (max 8).
    #[arg(long = "mcp-server")]
    pub servers: Vec<String>,
    #[arg(long = "mcp-profile")]
    pub profile: Option<String>,
    #[arg(long = "mcp-project", requires = "project_config")]
    pub project: Option<PathBuf>,
    #[arg(long = "mcp-project-config", requires = "project")]
    pub project_config: Option<PathBuf>,
}
impl Selection {
    pub fn project(&self) -> Result<Option<Project>, Error> {
        match (&self.project, &self.project_config) {
            (Some(root), Some(path)) => Ok(Some(Project::new(root, path)?)),
            (None, None) => Ok(None),
            _ => Err(Error::Configuration),
        }
    }
    pub fn validate(&self) -> Result<(), AgentError> {
        if self.servers.len() > 8
            || self
                .servers
                .iter()
                .any(|s| !namzu_mcp::config::valid_name(s))
            || self.servers.iter().collect::<BTreeSet<_>>().len() != self.servers.len()
            || self.servers.is_empty()
                && (self.profile.is_some()
                    || self.project.is_some()
                    || self.project_config.is_some())
        {
            return Err(AgentError::Configuration("invalid explicit MCP selection"));
        }
        Ok(())
    }
}
pub fn error(e: Error) -> AgentError {
    match e {
        Error::State(e) => e,
        _ => AgentError::Provider(e.to_string()),
    }
}
pub fn manager<'a>(
    registry: &'a Registry,
    selection: &Selection,
    ctx: &TurnContext,
) -> Result<Manager<'a>, AgentError> {
    selection.validate()?;
    Manager::new(
        registry,
        selection.profile.clone(),
        selection.project().map_err(error)?,
        Policy {
            require_approval: false,
            ..Policy::default()
        },
        ctx,
    )
    .map_err(error)
}
pub fn identity(
    state: &Path,
    selection: &Selection,
    ctx: &TurnContext,
) -> Result<Value, AgentError> {
    selection.validate()?;
    if selection.servers.is_empty() {
        return Ok(Value::Null);
    }
    let registry = Registry::open(state).map_err(error)?;
    let manager = manager(&registry, selection, ctx)?;
    Ok(json!({"selection":selection,"snapshot":manager.identity()}))
}
pub fn admit(
    plan: Plan,
    explicit: bool,
    events: &mut dyn EventSink,
    ctx: &TurnContext,
) -> Result<Permit, Error> {
    match plan.decision() {
        Decision::Allow => plan.permit(),
        Decision::Deny => Err(Error::Denied),
        Decision::Ask => {
            let approved = if events.supports_approval() {
                events.approve(&ToolCall{id:"mcp-bootstrap".into(),name:format!("mcp.setup.{}",plan.server()),arguments:json!({"server":plan.server(),"method":plan.method().unwrap_or("connect"),"params":plan.params()})},ctx)?
            } else {
                explicit
            };
            if approved {
                plan.approve()
            } else {
                Err(Error::ApprovalRequired)
            }
        }
    }
}
pub struct Admission<'a> {
    pub selection: &'a Selection,
    pub allow: Option<&'a BTreeSet<String>>,
    pub deny: &'a BTreeSet<String>,
    pub explicit: bool,
}
pub fn load<'a, L: Tools>(
    local: L,
    manager: Manager<'a>,
    admission: Admission<'_>,
    approvals: Approvals,
    events: &mut dyn EventSink,
    ctx: &TurnContext,
) -> Result<AgentTools<'a, L>, AgentError> {
    let mut tools = AgentTools::new(local, manager, approvals);
    if let Err(e) = tools.load(
        &admission.selection.servers,
        admission.allow,
        admission.deny,
        ctx,
        |plan| admit(plan, admission.explicit, events, ctx),
    ) {
        let cleanup = TurnContext::new(Duration::from_secs(5), Cancellation::default())?;
        let _ = tools.close(&cleanup);
        return Err(error(e));
    }
    Ok(tools)
}
