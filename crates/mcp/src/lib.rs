//! Transport-independent MCP client protocol. This package performs no I/O,
//! executes no server tools and never retries an uncertain request.
mod session;
pub mod types;
pub mod wire;
pub use session::{Abandoned, ClientSession, Expired, Incoming, Phase};
pub use wire::{Id, Message, RpcError};

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("invalid MCP protocol limits")]
    Limits,
    #[error("MCP message exceeds configured limits")]
    MessageLimit,
    #[error("invalid or structurally excessive MCP JSON")]
    Json,
    #[error("invalid MCP envelope: {0}")]
    Envelope(&'static str),
    #[error("MCP operation is invalid in this lifecycle phase")]
    Phase,
    #[error("MCP request limit reached")]
    RequestLimit,
    #[error("MCP response has an unknown or already completed ID")]
    ResponseId,
    #[error("invalid MCP initialize result or unsupported protocol version")]
    Initialize,
    #[error("MCP capability or operation was not enabled")]
    Capability,
    #[error("MCP initialize must not be cancelled")]
    CancelInitialize,
    #[error("MCP request context ended: {0}")]
    Context(namzu_core::AgentError),
}
impl From<namzu_core::AgentError> for Error {
    fn from(value: namzu_core::AgentError) -> Self {
        Self::Context(value)
    }
}

/// Local resource ceilings, not peer-negotiated permissions. IDs are additionally
/// limited to 256 UTF-8 bytes and interoperable integer values by the codec.
#[derive(Clone, Debug)]
pub struct Limits {
    pub message_bytes: usize,
    pub string_bytes: usize,
    pub depth: usize,
    pub nodes: usize,
    pub pending: usize,
    pub requests: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            message_bytes: 1024 * 1024,
            string_bytes: 512 * 1024,
            depth: 48,
            nodes: 32768,
            pending: 64,
            requests: 4096,
        }
    }
}
impl Limits {
    pub fn validate(&self) -> Result<(), Error> {
        if !(256..=4 * 1024 * 1024).contains(&self.message_bytes)
            || !(1..=self.message_bytes).contains(&self.string_bytes)
            || !(1..=64).contains(&self.depth)
            || !(1..=65536).contains(&self.nodes)
            || !(1..=256).contains(&self.pending)
            || !(1..=65536).contains(&self.requests)
            || self.pending > self.requests
        {
            return Err(Error::Limits);
        }
        Ok(())
    }
}
