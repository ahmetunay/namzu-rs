//! Optional capability registry and host tool composition. Core has no package dependency.
mod tools;
use namzu_core::AgentError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::PathBuf};
pub use tools::{CapabilityTools, Speech};
pub const PACKAGES: &[&str] = &["files", "audio", "lsp", "sandbox"];
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AudioConfig {
    pub base_url: String,
    pub api_key_env: String,
    pub transcription_model: String,
    pub speech_model: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LspConfig {
    pub program: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    /// Names only: values are resolved on an explicitly authorized call and never persisted here.
    #[serde(default)]
    pub env: BTreeSet<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub allow: Option<BTreeSet<String>>,
    pub deny: BTreeSet<String>,
    pub audio: Option<AudioConfig>,
    pub lsp: Option<LspConfig>,
}
fn env_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
fn bad() -> AgentError {
    AgentError::Configuration("invalid capability configuration or unsupported selection")
}
impl Config {
    pub fn permits(&self, name: &str) -> bool {
        !self.deny.contains(name) && self.allow.as_ref().is_none_or(|s| s.contains(name))
    }
    pub fn validate(&self) -> Result<(), AgentError> {
        if self
            .deny
            .iter()
            .chain(self.allow.iter().flatten())
            .any(|s| !PACKAGES.contains(&s.as_str()))
        {
            return Err(bad());
        }
        if let Some(a) = &self.audio {
            let url = url::Url::parse(&a.base_url).map_err(|_| bad())?;
            let loopback = match url.host() {
                Some(url::Host::Ipv4(v)) => v.is_loopback(),
                Some(url::Host::Ipv6(v)) => v.is_loopback(),
                _ => url.host_str() == Some("localhost"),
            };
            if !(url.scheme() == "https" || url.scheme() == "http" && loopback)
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || a.base_url.len() > 2048
                || !env_name(&a.api_key_env)
                || [&a.transcription_model, &a.speech_model].iter().any(|s| {
                    s.is_empty() || s.len() > 256 || !s.bytes().all(|b| (33..=126).contains(&b))
                })
            {
                return Err(bad());
            }
        }
        if let Some(l) = &self.lsp
            && (!l.program.is_absolute()
                || l.args.len() > 128
                || l.args.iter().map(String::len).sum::<usize>() > 65536
                || l.args.iter().any(|s| s.contains('\0'))
                || l.env.len() > 64
                || l.env.iter().any(|s| !env_name(s)))
        {
            return Err(bad());
        }
        Ok(())
    }
    /// Stable identity of selected settings. Per-invocation grants are deliberately not saved.
    pub fn identity(&self, selected: &BTreeSet<String>) -> Result<Value, AgentError> {
        self.validate()?;
        if selected
            .iter()
            .any(|s| !["audio", "lsp"].contains(&s.as_str()) || !self.permits(s))
        {
            return Err(bad());
        }
        if selected.contains("lsp") && self.lsp.is_none() {
            return Err(AgentError::Configuration(
                "selected LSP capability requires explicit server configuration",
            ));
        }
        if selected.is_empty() && self.permits("files") {
            return Ok(Value::Null);
        }
        let value = json!({"version":1,"selected":selected,"files":self.permits("files"),"audio":if selected.contains("audio") {self.audio.as_ref()}else{None},"lsp":if selected.contains("lsp") {self.lsp.as_ref()}else{None}});
        let hash = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&value).map_err(|_| bad())?)
        );
        Ok(json!({"settings":value,"sha256":hash}))
    }
    pub fn registry(&self, selected: &BTreeSet<String>, guest: bool) -> Result<Value, AgentError> {
        self.identity(selected)?;
        if guest && !selected.is_empty() {
            return Err(AgentError::Configuration(
                "audio/LSP capabilities require a host workspace; guest selection never falls back to host",
            ));
        }
        Ok(
            json!({"version":1,"packages":PACKAGES.iter().map(|name|json!({"name":name,"permitted":self.permits(name),"selected":match *name {"files"=>self.permits(name),"sandbox"=>guest,_=>selected.contains(*name)},"runtime":if guest {"guest"}else{"host"}})).collect::<Vec<_>>(),"identity":self.identity(selected)?}),
        )
    }
}
