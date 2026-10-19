//! Explicit per-invocation visual ceiling; profiles never manufacture grants.
use clap::Args;
use namzu_computer::{ComputerManager, ComputerSpec, Docker, VisualPolicy};
use namzu_core::AgentError;
use namzu_session::PrivateStore;
use std::path::Path;
#[derive(Clone, Default, Args)]
pub struct VisualSelection {
    /// Explicit bounded JSON sandbox policy; requires a pinned guest image.
    #[arg(long)]
    pub sandbox_policy: Option<std::path::PathBuf>,
    #[arg(id = "computer_image", long = "computer-image")]
    pub image: Option<String>,
    #[arg(id = "computer_desktop", long = "computer-desktop")]
    pub desktop: bool,
    #[arg(id = "computer_browser", long = "computer-browser")]
    pub browser: bool,
    #[arg(id = "visual_read", long = "visual-read")]
    pub observe: bool,
    #[arg(id = "visual_input", long = "visual-input")]
    pub input: bool,
    #[arg(id = "visual_navigate", long = "visual-navigate")]
    pub navigate: bool,
    #[arg(id = "visual_actions", long = "visual-actions")]
    pub actions: bool,
    #[arg(id = "visual_upload", long = "visual-upload")]
    pub upload: bool,
    #[arg(id = "visual_download", long = "visual-download")]
    pub download: bool,
}
impl VisualSelection {
    pub fn policy(&self) -> VisualPolicy {
        VisualPolicy {
            observe: self.observe,
            input: self.input,
            navigate: self.navigate,
            actions: self.actions,
            upload: self.upload,
            download: self.download,
        }
    }
    pub fn spec(&self) -> Result<Option<ComputerSpec>, AgentError> {
        match &self.image {
            Some(image) => Ok(Some(self.spec_for(image)?)),
            None => {
                if self.desktop || self.browser || self.sandbox_policy.is_some() {
                    return Err(AgentError::Configuration(
                        "--computer-image required for computer hardware flags",
                    ));
                }
                Ok(None)
            }
        }
    }
    pub fn spec_for(&self, image: &str) -> Result<ComputerSpec, AgentError> {
        let mut spec = ComputerSpec::new(image);
        if self.desktop || self.browser {
            spec.desktop = Some(Default::default());
        }
        if self.browser {
            spec.browser = true;
            spec.memory_bytes = 768 * 1024 * 1024;
            spec.pids = 128;
        }
        self.apply_sandbox(spec)
    }
    pub fn apply_sandbox(&self, mut spec: ComputerSpec) -> Result<ComputerSpec, AgentError> {
        if let Some(path) = &self.sandbox_policy {
            use std::io::Read;
            let file = std::fs::File::open(path)
                .map_err(|_| AgentError::Configuration("cannot open selected sandbox policy"))?;
            let mut bytes = vec![];
            file.take(65537)
                .read_to_end(&mut bytes)
                .map_err(|_| AgentError::Configuration("cannot read selected sandbox policy"))?;
            spec = spec.with_sandbox(namzu_sandbox::Policy::parse(&bytes)?)?;
        }
        spec.validate()?;
        Ok(spec)
    }
    pub fn description(&self) -> String {
        format!(
            "{} visual:{}{}{}{}{}{}",
            if self.sandbox_policy.is_some() {
                "sandbox policy selected"
            } else {
                ""
            },
            if self.observe { " read" } else { "" },
            if self.input { " input" } else { "" },
            if self.navigate { " navigate" } else { "" },
            if self.actions { " actions" } else { "" },
            if self.upload { " upload" } else { "" },
            if self.download { " download" } else { "" }
        )
    }
}
pub fn manager(state: &Path, id: &str) -> Result<ComputerManager, AgentError> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        return Err(AgentError::Configuration("invalid computer session id"));
    }
    ComputerManager::from_store(
        PrivateStore::open(state.join("exec-computers").join(id))?,
        "exec",
        id,
        Docker::local("/usr/local/bin/docker")?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_pinned_hardware_and_independent_effect_grants() {
        assert!(
            VisualSelection {
                browser: true,
                ..Default::default()
            }
            .spec()
            .is_err()
        );
        assert!(
            VisualSelection {
                image: Some("latest".into()),
                ..Default::default()
            }
            .spec()
            .is_err()
        );
        let selection = VisualSelection {
            image: Some(format!("sha256:{}", "a".repeat(64))),
            browser: true,
            observe: true,
            ..Default::default()
        };
        let spec = selection.spec().unwrap().unwrap();
        assert!(spec.browser && spec.desktop.is_some() && spec.pids == 128);
        let policy = selection.policy();
        assert!(
            policy.observe
                && !policy.actions
                && !policy.input
                && !policy.upload
                && !policy.download
        );
        assert_eq!(selection.spec_for(&spec.image).unwrap(), spec);
    }
}
