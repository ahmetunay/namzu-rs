//! Explicit, bounded policies for owned offline Docker sandboxes.
//! Validation never probes or falls back to an unsandboxed host process.
use namzu_core::AgentError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    Docker,
    Native,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Workspace {
    ReadOnly,
    ReadWrite,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Execution {
    ToolsOnly,
    Commands,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Network {
    Disabled,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub memory_bytes: u64,
    pub cpu_millis: u32,
    pub pids: u32,
    /// RLIMIT_FSIZE: per-file limit, not a total workspace quota.
    pub file_bytes: u64,
    pub open_files: u32,
    pub scratch_bytes: u64,
    pub timeout_seconds: u64,
    pub output_bytes: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub version: u32,
    pub backend: Backend,
    pub workspace: Workspace,
    pub execution: Execution,
    pub network: Network,
    pub limits: Limits,
}
impl Policy {
    pub fn inspect() -> Self {
        Self {
            version: 1,
            backend: Backend::Docker,
            workspace: Workspace::ReadOnly,
            execution: Execution::ToolsOnly,
            network: Network::Disabled,
            limits: Limits {
                memory_bytes: 256 * 1024 * 1024,
                cpu_millis: 1000,
                pids: 64,
                file_bytes: 16 * 1024 * 1024,
                open_files: 256,
                scratch_bytes: 16 * 1024 * 1024,
                timeout_seconds: 300,
                output_bytes: 64 * 1024,
            },
        }
    }
    pub fn readonly() -> Self {
        Self {
            execution: Execution::Commands,
            ..Self::inspect()
        }
    }
    pub fn build() -> Self {
        Self {
            workspace: Workspace::ReadWrite,
            ..Self::readonly()
        }
    }
    pub fn validate(&self) -> Result<(), AgentError> {
        if self.version != 1 || self.backend != Backend::Docker {
            return Err(AgentError::Configuration(
                "sandbox requires policy v1 and local Docker; native backend unsupported, no host fallback",
            ));
        }
        let l = &self.limits;
        if !(32 * 1024 * 1024..=2 * 1024 * 1024 * 1024).contains(&l.memory_bytes)
            || !(100..=4000).contains(&l.cpu_millis)
            || !(16..=256).contains(&l.pids)
            || !(1024..=1024 * 1024 * 1024).contains(&l.file_bytes)
            || !(32..=4096).contains(&l.open_files)
            || !(1024 * 1024..=256 * 1024 * 1024).contains(&l.scratch_bytes)
            || !(1..=300).contains(&l.timeout_seconds)
            || !(1..=64 * 1024).contains(&l.output_bytes)
        {
            return Err(AgentError::Configuration(
                "sandbox resource limits out of bounds",
            ));
        }
        Ok(())
    }
    pub fn writable(&self) -> bool {
        self.workspace == Workspace::ReadWrite
    }
    pub fn commands(&self) -> bool {
        self.execution == Execution::Commands
    }
    pub fn check_write(&self, bytes: usize) -> Result<(), AgentError> {
        if !self.writable() || bytes as u64 > self.limits.file_bytes {
            return Err(AgentError::Configuration(
                "sandbox denies write or file exceeds policy limit",
            ));
        }
        Ok(())
    }
    pub fn check_command(&self) -> Result<(), AgentError> {
        if !self.commands() {
            return Err(AgentError::Configuration(
                "sandbox command execution denied",
            ));
        }
        Ok(())
    }
    pub fn fingerprint(&self) -> Result<String, AgentError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)
            .map_err(|_| AgentError::Protocol("sandbox policy encoding"))?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
    /// Parse an explicitly selected, bounded policy; no ambient config discovery.
    pub fn parse(bytes: &[u8]) -> Result<Self, AgentError> {
        if bytes.len() > 64 * 1024 {
            return Err(AgentError::Configuration("sandbox policy too large"));
        }
        let policy: Self = serde_json::from_slice(bytes)
            .map_err(|_| AgentError::Configuration("invalid sandbox policy"))?;
        policy.validate()?;
        Ok(policy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profiles_and_canonical_identity() {
        let p = Policy::build();
        let bytes = serde_json::to_vec_pretty(&p).unwrap();
        let copy = Policy::parse(&bytes).unwrap();
        assert_eq!(p, copy);
        assert_eq!(p.fingerprint().unwrap(), copy.fingerprint().unwrap());
        assert_ne!(
            p.fingerprint().unwrap(),
            Policy::readonly().fingerprint().unwrap()
        );
        assert!(Policy::inspect().check_command().is_err());
        assert!(Policy::readonly().check_write(0).is_err());
        assert!(p.check_write(p.limits.file_bytes as usize + 1).is_err());
    }
    #[test]
    fn rejects_native_unknown_network_and_unknown_fields() {
        let mut p = Policy::inspect();
        p.backend = Backend::Native;
        assert!(p.validate().is_err());
        for (key, value) in [
            ("network", serde_json::json!("internet")),
            ("version", serde_json::json!(2)),
            ("surprise", serde_json::json!(true)),
        ] {
            let mut v = serde_json::to_value(Policy::inspect()).unwrap();
            v[key] = value;
            assert!(Policy::parse(&serde_json::to_vec(&v).unwrap()).is_err());
        }
        assert!(Policy::parse(&vec![b' '; 65537]).is_err());
    }
    #[test]
    fn every_limit_is_bounded() {
        let base = serde_json::to_value(Policy::build()).unwrap();
        for key in [
            "memory_bytes",
            "cpu_millis",
            "pids",
            "file_bytes",
            "open_files",
            "scratch_bytes",
            "timeout_seconds",
            "output_bytes",
        ] {
            for value in [0u64, u64::MAX] {
                let mut v = base.clone();
                v["limits"][key] = value.into();
                assert!(
                    Policy::parse(&serde_json::to_vec(&v).unwrap()).is_err(),
                    "{key}"
                );
            }
        }
    }
}
