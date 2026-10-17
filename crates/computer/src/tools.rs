use crate::{Computer, Engine};
use namzu_core::{AgentError, ToolCall, ToolDefinition, ToolResult, Tools, TurnContext};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;
pub type GuestPolicy = namzu_tools::ToolPolicy;
pub struct GuestTools<'a, E: Engine> {
    computer: Computer<'a, E>,
    policy: GuestPolicy,
}
impl<'a, E: Engine> GuestTools<'a, E> {
    pub fn new(computer: Computer<'a, E>, policy: GuestPolicy) -> Self {
        Self { computer, policy }
    }
    pub fn computer_mut(&mut self) -> &mut Computer<'a, E> {
        &mut self.computer
    }
    pub fn into_computer(self) -> Computer<'a, E> {
        self.computer
    }
    fn admitted(&self, name: &str) -> bool {
        !self.policy.deny.contains(name)
            && self.policy.allow.as_ref().is_none_or(|s| s.contains(name))
            && match name {
                "read_file" | "list_files" => true,
                "write_file" => self.policy.allow_write,
                "shell" => self.policy.allow_shell,
                _ => false,
            }
    }
    fn execute(&mut self, call: &ToolCall, ctx: &TurnContext) -> Result<ToolResult, AgentError> {
        macro_rules! args {
            ($t:ty) => {
                match serde_json::from_value::<$t>(call.arguments.clone()) {
                    Ok(v) => v,
                    Err(_) => return Ok(ToolResult::error("invalid guest tool arguments")),
                }
            };
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Path {
            path: String,
        }
        match call.name.as_str() {
            "read_file" => {
                let a = args!(Path);
                let bytes = self.computer.download(&a.path, ctx)?;
                Ok(match String::from_utf8(bytes) {
                    Ok(text) => ToolResult::ok(text),
                    Err(_) => ToolResult::error("binary file: use host binary transfer API"),
                })
            }
            "list_files" => {
                let a = args!(Path);
                Ok(ToolResult::ok(
                    json!(self.computer.list(&a.path, ctx)?).to_string(),
                ))
            }
            "write_file" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Write {
                    path: String,
                    content: String,
                }
                let a = args!(Write);
                self.computer.upload(&a.path, a.content.as_bytes(), ctx)?;
                Ok(ToolResult::ok("written in owned guest workspace"))
            }
            "shell" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Shell {
                    command: String,
                    #[serde(default = "dot")]
                    cwd: String,
                    #[serde(default = "timeout")]
                    timeout_seconds: u64,
                }
                let a = args!(Shell);
                let output = self.computer.shell(
                    &a.command,
                    &a.cwd,
                    Duration::from_secs(a.timeout_seconds),
                    ctx,
                )?;
                Ok(ToolResult {
                    is_error: output.status != "exited" || output.exit_code != Some(0),
                    content: serde_json::to_string(&output)
                        .map_err(|_| AgentError::Protocol("cannot encode guest process output"))?,
                })
            }
            _ => Ok(ToolResult::error("guest tool denied or unavailable")),
        }
    }
}
fn dot() -> String {
    ".".into()
}
fn timeout() -> u64 {
    60
}
impl<E: Engine> Tools for GuestTools<'_, E> {
    fn definitions(&self) -> Vec<ToolDefinition> {
        [
            ("read_file","Read a bounded UTF-8 file in the owned guest workspace.",json!({"path":{"type":"string"}}),vec!["path"]),
            ("list_files","List the owned guest workspace directory.",json!({"path":{"type":"string"}}),vec!["path"]),
            ("write_file","Atomically replace a bounded file in the owned guest workspace.",json!({"path":{"type":"string"},"content":{"type":"string"}}),vec!["path","content"]),
            ("shell","Run /bin/sh inside the owned offline container; never falls back to host shell.",json!({"command":{"type":"string"},"cwd":{"type":"string"},"timeout_seconds":{"type":"integer","minimum":1,"maximum":300}}),vec!["command"]),
        ].into_iter().filter(|(name,..)|self.admitted(name)).map(|(name,description,properties,required)|ToolDefinition{
            name:name.into(),description:description.into(),parameters:json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
        }).collect()
    }
    fn call(&mut self, call: &ToolCall, ctx: &TurnContext) -> Result<ToolResult, AgentError> {
        ctx.remaining()?;
        if !self.admitted(&call.name) {
            return Ok(ToolResult::error("guest tool denied or unavailable"));
        }
        match self.execute(call, ctx) {
            Err(error @ AgentError::Cancelled) | Err(error @ AgentError::Timeout) => Err(error),
            Err(_) => Ok(ToolResult::error(
                "guest operation failed; inspect computer state; no host fallback",
            )),
            result => result,
        }
    }
}
