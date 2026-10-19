use super::*;
use namzu_core::{ToolCall, ToolDefinition, ToolResult, Tools};
use std::{cell::RefCell, io::Write, rc::Rc};
pub struct SharedGuestTools<'a>(Rc<RefCell<GuestTools<'a, Docker>>>);
impl Clone for SharedGuestTools<'_> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<'a> SharedGuestTools<'a> {
    pub fn new(tools: GuestTools<'a, Docker>) -> Self {
        Self(Rc::new(RefCell::new(tools)))
    }
    pub fn launcher(&self) -> Box<dyn namzu_mcp::manager::GuestStdio + 'a> {
        Box::new(Bridge {
            tools: self.0.clone(),
            env_files: Vec::new(),
        })
    }
    /// Kill all remote exec descendants before acknowledging a healthy MCP scope.
    /// An interrupted scope remains pending for explicit operator inspection.
    pub fn finish_mcp(&self, healthy: bool, ctx: &TurnContext) -> Result<(), AgentError> {
        let mut tools = self
            .0
            .try_borrow_mut()
            .map_err(|_| AgentError::State("guest busy"))?;
        let computer = tools.computer_mut();
        let active = computer.mcp_active;
        computer.stop(ctx)?;
        if healthy
            && active
            && computer
                .record
                .pending
                .as_ref()
                .is_some_and(|p| p.kind == "mcp-active")
        {
            computer.record.pending = None;
            computer.manager.save(&computer.record)?;
        }
        Ok(())
    }
    pub fn into_computer(self) -> Result<Computer<'a, Docker>, AgentError> {
        Rc::try_unwrap(self.0)
            .map_err(|_| AgentError::State("guest MCP owner still active"))
            .map(|r| r.into_inner().into_computer())
    }
}
impl Tools for SharedGuestTools<'_> {
    fn validate(&mut self, call: &ToolCall, ctx: &TurnContext) -> Result<(), AgentError> {
        self.0
            .try_borrow_mut()
            .map_err(|_| AgentError::State("guest busy"))?
            .validate(call, ctx)
    }
    fn definitions(&self) -> Vec<ToolDefinition> {
        self.0.borrow().definitions()
    }
    fn call(&mut self, call: &ToolCall, ctx: &TurnContext) -> Result<ToolResult, AgentError> {
        self.0
            .try_borrow_mut()
            .map_err(|_| AgentError::State("guest busy"))?
            .call(call, ctx)
    }
}
struct Bridge<'a> {
    tools: Rc<RefCell<GuestTools<'a, Docker>>>,
    env_files: Vec<tempfile::NamedTempFile>,
}
impl Computer<'_, Docker> {
    fn check_mcp(&self, ctx: &TurnContext) -> Result<String, AgentError> {
        if let Some(policy) = &self.record.spec.sandbox {
            policy.check_command()?;
        }
        ctx.remaining()?;
        if self.record.phase != Phase::Running
            || self
                .record
                .pending
                .as_ref()
                .is_some_and(|p| p.kind != "mcp-active" || !self.mcp_active)
        {
            return Err(AgentError::State(
                "guest unavailable or uncertain; no host fallback",
            ));
        }
        let info = self
            .manager
            .owned_container(&self.record, ctx)?
            .filter(|c| c.running && c.confined(&self.record.spec, &self.record.volume))
            .ok_or(AgentError::State("guest ownership or confinement changed"))?;
        Ok(info.id)
    }
}
impl namzu_mcp::manager::GuestStdio for Bridge<'_> {
    fn recovery_scope(&self) -> String {
        self.tools.borrow().computer().snapshot().container.clone()
    }

    fn check(&mut self, ctx: &TurnContext) -> Result<(), namzu_mcp::config::Error> {
        self.tools
            .try_borrow()
            .map_err(|_| AgentError::State("guest busy"))?
            .computer()
            .check_mcp(ctx)?;
        Ok(())
    }
    fn launch(
        &mut self,
        mut spec: namzu_mcp::stdio::Spec,
        ctx: &TurnContext,
    ) -> Result<namzu_mcp::stdio::Spec, namzu_mcp::config::Error> {
        if !spec.program.is_absolute() || !spec.cwd.is_absolute() {
            return Err(namzu_mcp::config::Error::Configuration);
        }
        let mut tools = self
            .tools
            .try_borrow_mut()
            .map_err(|_| AgentError::State("guest busy"))?;
        let computer = tools.computer_mut();
        let id = computer.check_mcp(ctx)?;
        let docker = computer
            .manager
            .engine
            .lock()
            .map_err(|_| AgentError::State("computer engine busy"))?
            .executable();
        let mut args = vec![
            "--host=unix:///var/run/docker.sock".into(),
            "exec".into(),
            "--interactive".into(),
            "--workdir".into(),
            spec.cwd.to_string_lossy().into_owned(),
        ];
        if !spec.env.is_empty() {
            // Guest-selected env must never change the host Docker client
            // (DOCKER_HOST, LD_PRELOAD, HOME, PATH, proxy selectors, ...).
            // NamedTempFile is private 0600 and is unlinked with this scope.
            let mut file = tempfile::NamedTempFile::new()
                .map_err(|_| AgentError::State("cannot create private guest env file"))?;
            for (key, value) in &spec.env {
                if value.contains(['\n', '\r', '\0']) {
                    return Err(namzu_mcp::config::Error::Configuration);
                }
                writeln!(file, "{key}={value}")
                    .map_err(|_| AgentError::State("cannot write private guest env file"))?;
            }
            file.flush()
                .map_err(|_| AgentError::State("cannot flush private guest env file"))?;
            args.extend([
                "--env-file".into(),
                file.path().to_string_lossy().into_owned(),
            ]);
            self.env_files.push(file);
        }
        spec.env.clear();
        args.extend([id, spec.program.to_string_lossy().into_owned()]);
        args.extend(spec.args);
        computer.record.pending = Some(Pending {
            kind: "mcp-active".into(),
            started_ms: now_ms(),
        });
        computer.manager.save(&computer.record)?;
        computer.mcp_active = true;
        spec.program = docker;
        spec.args = args;
        spec.cwd = "/".into();
        Ok(spec)
    }
}
