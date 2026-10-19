use namzu_core::AgentError;
use namzu_session::PrivateStore;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, io::Read, path::Path};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub capabilities: namzu_capabilities::Config,
    pub provider: String,
    pub model: Option<String>,
    pub base_url: String,
    pub api_key_env: String,
    pub timeout: u64,
    pub max_steps: usize,
    pub max_tool_calls: usize,
    pub max_text_bytes: usize,
    pub tool_allow: Option<BTreeSet<String>>,
    pub tool_deny: BTreeSet<String>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            capabilities: Default::default(),
            provider: "openai-compatible".into(),
            model: None,
            base_url: "https://api.openai.com/v1".into(),
            api_key_env: "OPENAI_API_KEY".into(),
            timeout: 300,
            max_steps: 64,
            max_tool_calls: 128,
            max_text_bytes: 4 * 1024 * 1024,
            tool_allow: None,
            tool_deny: BTreeSet::new(),
        }
    }
}
impl Config {
    pub fn validate(&self) -> Result<(), AgentError> {
        self.capabilities.validate()?;
        if self.version != 1
            || !(1..=86400).contains(&self.timeout)
            || self.max_steps == 0
            || self.max_steps > 10000
            || self.max_tool_calls > 10000
            || !(1..=4 * 1024 * 1024).contains(&self.max_text_bytes)
            || !["openai-compatible", "codex"].contains(&self.provider.as_str())
            || self
                .model
                .as_ref()
                .is_some_and(|m| m.is_empty() || m.len() > 256)
            || self.api_key_env.is_empty()
            || !self
                .api_key_env
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(AgentError::Configuration(
                "invalid config fields, version or budgets",
            ));
        }
        Ok(())
    }
    pub fn load(store: &PrivateStore, explicit: Option<&Path>) -> Result<Self, AgentError> {
        let config: Self = if let Some(path) = explicit {
            let file = std::fs::File::open(path)
                .map_err(|_| AgentError::Configuration("cannot open selected config"))?;
            let mut bytes = vec![];
            file.take(65537)
                .read_to_end(&mut bytes)
                .map_err(|_| AgentError::Configuration("cannot read selected config"))?;
            if bytes.len() > 65536 {
                return Err(AgentError::Configuration("config exceeds 64 KiB"));
            }
            serde_json::from_slice(&bytes)
                .map_err(|_| AgentError::Configuration("invalid selected config JSON"))?
        } else {
            store.read("config.json")?.unwrap_or_default()
        };
        config.validate()?;
        Ok(config)
    }
}
