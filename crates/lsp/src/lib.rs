//! Optional bounded LSP SDK. Explicit host launch; no project auto-discovery or automatic edits.
mod workspace;
pub use workspace::{
    Applied, DOCUMENT_BYTES, Document, EditPlan, Position, Range, TextEdit, Workspace, apply_text,
    digest, offset,
};
#[cfg(unix)]
mod client;
#[cfg(unix)]
mod process;
#[cfg(unix)]
mod transport;
#[cfg(unix)]
pub use client::{Client, Diagnostic, Location, plan_from_edit};
use std::{collections::BTreeMap, path::PathBuf};
#[cfg(unix)]
pub use transport::CloseReport;
pub struct Spec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
}
impl Spec {
    pub fn new(program: impl Into<PathBuf>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            cwd: cwd.into(),
            args: vec![],
            env: BTreeMap::new(),
        }
    }
    pub(crate) fn validate(&self) -> Result<(), namzu_core::AgentError> {
        if !self.program.is_absolute()
            || !self.program.is_file()
            || !self.cwd.is_absolute()
            || !self.cwd.is_dir()
            || self.args.len() > 128
            || self.args.iter().map(String::len).sum::<usize>() > 65536
            || self.args.iter().any(|s| s.contains('\0'))
            || self.env.len() > 64
            || self.env.iter().any(|(k, v)| {
                k.is_empty()
                    || k.len() > 128
                    || !k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    || v.len() > 16384
                    || v.contains('\0')
            })
        {
            return Err(namzu_core::AgentError::Configuration(
                "invalid explicit LSP launch configuration",
            ));
        }
        Ok(())
    }
}
