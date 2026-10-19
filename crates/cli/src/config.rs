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
    #[serde(
        default,
        skip_serializing_if = "namzu_provider::services::Options::is_default"
    )]
    pub api_options: namzu_provider::services::Options,
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
            api_options: Default::default(),
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
        self.api_options.validate()?;
        if self.provider == "codex" && !self.api_options.is_default() {
            return Err(AgentError::Configuration(
                "Codex does not accept API service options",
            ));
        }
        if self.version != 1
            || !(1..=86400).contains(&self.timeout)
            || self.max_steps == 0
            || self.max_steps > 10000
            || self.max_tool_calls > 10000
            || !(1..=4 * 1024 * 1024).contains(&self.max_text_bytes)
            || ![
                "openai-compatible",
                "openrouter",
                "zen",
                "zen-go",
                "codex",
                "anthropic",
                "google",
                "deepseek",
                "bedrock",
                "lmstudio",
                "ollama",
            ]
            .contains(&self.provider.as_str())
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
        let raw: Option<Box<serde_json::value::RawValue>> = if let Some(path) = explicit {
            let file = std::fs::File::open(path)
                .map_err(|_| AgentError::Configuration("cannot open selected config"))?;
            let mut bytes = vec![];
            file.take(65537)
                .read_to_end(&mut bytes)
                .map_err(|_| AgentError::Configuration("cannot read selected config"))?;
            if bytes.len() > 65536 {
                return Err(AgentError::Configuration("config exceeds 64 KiB"));
            }
            Some(
                serde_json::from_slice(&bytes)
                    .map_err(|_| AgentError::Configuration("invalid selected config JSON"))?,
            )
        } else {
            store.read("config.json")?
        };
        let mut config: Self = raw
            .as_ref()
            .map(|value| {
                serde_json::from_str(value.get())
                    .map_err(|_| AgentError::Configuration("invalid selected config JSON"))
            })
            .transpose()?
            .unwrap_or_default();
        let fields = raw
            .as_ref()
            .map(|v| serde_json::from_str::<serde_json::Value>(v.get()))
            .transpose()
            .map_err(|_| AgentError::Configuration("invalid selected config JSON"))?;
        if config.provider != "codex" {
            let service = namzu_provider::services::Service::parse(&config.provider)?;
            if fields.as_ref().is_some_and(|v| v.get("base_url").is_none()) {
                config.base_url = service.base_url().into();
            }
            if fields
                .as_ref()
                .is_some_and(|v| v.get("api_key_env").is_none())
            {
                config.api_key_env = service.key_env().into();
            }
        }
        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn service_config_selects_its_own_default_endpoint_and_environment_name() {
        let dir = tempfile::tempdir().unwrap();
        let store = PrivateStore::open(dir.path().join("state")).unwrap();
        for name in [
            "openrouter",
            "zen",
            "zen-go",
            "anthropic",
            "google",
            "deepseek",
            "bedrock",
            "lmstudio",
            "ollama",
        ] {
            let path = dir.path().join("config.json");
            std::fs::write(&path, serde_json::json!({"provider":name}).to_string()).unwrap();
            let c = Config::load(&store, Some(&path)).unwrap();
            let service = namzu_provider::services::Service::parse(name).unwrap();
            assert_eq!(c.base_url, service.base_url());
            assert_eq!(c.api_key_env, service.key_env());
        }
    }
    #[test]
    fn service_config_preserves_explicit_selectors_and_rejects_unknown_options() {
        let dir = tempfile::tempdir().unwrap();
        let store = PrivateStore::open(dir.path().join("state")).unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path,serde_json::json!({"provider":"openrouter","base_url":"http://127.0.0.1:1234/v1","api_key_env":"FIXTURE","api_options":{"stream":true}}).to_string()).unwrap();
        let c = Config::load(&store, Some(&path)).unwrap();
        assert_eq!(c.api_key_env, "FIXTURE");
        assert!(c.base_url.contains("127.0.0.1"));
        assert!(c.api_options.stream);
        std::fs::write(
            &path,
            serde_json::json!({"provider":"openrouter","api_options":{"fallback_models":["paid"]}})
                .to_string(),
        )
        .unwrap();
        assert!(Config::load(&store, Some(&path)).is_err());
        std::fs::write(&path, r#"{"provider":"zen","provider":"openrouter"}"#).unwrap();
        assert!(Config::load(&store, Some(&path)).is_err());
    }
}
