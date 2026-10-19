//! Bounded Chromium inspection in an explicitly owned offline guest.
use namzu_core::{AgentError, TurnContext};
use serde::{Deserialize, Serialize};
pub mod cdp;
pub const MAX_NODES: usize = 256;
pub const TEXT_BUDGET: usize = 64 * 1024;
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Info,
    Navigate {
        url: String,
    },
    Snapshot,
    Click {
        target: Target,
    },
    Fill {
        target: Target,
        text: String,
    },
    Select {
        target: Target,
        values: Vec<String>,
    },
    Press {
        target: Target,
        key: String,
    },
    ScrollIntoView {
        target: Target,
    },
    Upload {
        target: Target,
        path: String,
    },
    Download {
        document: Document,
        url: String,
        path: String,
    },
}
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BrowserRequest(<redacted>)")
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    Inspect,
    Navigate,
    Act,
    Upload,
    Download,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub frame_id: String,
    pub loader_id: String,
    pub url: String,
}
impl From<&Page> for Document {
    fn from(p: &Page) -> Self {
        Self {
            frame_id: p.frame_id.clone(),
            loader_id: p.loader_id.clone(),
            url: p.url.clone(),
        }
    }
}
impl Document {
    pub fn validate(&self) -> Result<(), AgentError> {
        if self.frame_id.is_empty()
            || self.frame_id.len() > 128
            || self.loader_id.is_empty()
            || self.loader_id.len() > 128
        {
            return Err(AgentError::Configuration(
                "bounded document identity required",
            ));
        }
        validate_url(&self.url)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub document: Document,
    pub backend_node_id: i64,
}
impl Target {
    pub fn validate(&self) -> Result<(), AgentError> {
        self.document.validate()?;
        if self.backend_node_id <= 0 {
            return Err(AgentError::Configuration(
                "positive backend node ID required",
            ));
        }
        Ok(())
    }
}
pub fn transfer_path(path: &str) -> bool {
    crate::protocol::valid_path(path)
        && std::path::Path::new(path).file_name().is_some_and(|s| {
            s.to_str().is_some_and(|s| {
                !s.is_empty() && s.len() <= 255 && !s.chars().any(char::is_control)
            })
        })
}
pub fn press_key(key: &str) -> Option<(&'static str, u32)> {
    Some(match key {
        "ENTER" => ("Enter", 13),
        "TAB" => ("Tab", 9),
        "ESC" => ("Escape", 27),
        "BACKSPACE" => ("Backspace", 8),
        "DELETE" => ("Delete", 46),
        "LEFT" => ("ArrowLeft", 37),
        "UP" => ("ArrowUp", 38),
        "RIGHT" => ("ArrowRight", 39),
        "DOWN" => ("ArrowDown", 40),
        "HOME" => ("Home", 36),
        "END" => ("End", 35),
        "PAGEUP" => ("PageUp", 33),
        "PAGEDOWN" => ("PageDown", 34),
        _ => return None,
    })
}
impl Request {
    pub fn permission(&self) -> Permission {
        match self {
            Self::Info | Self::Snapshot => Permission::Inspect,
            Self::Navigate { .. } => Permission::Navigate,
            Self::Upload { .. } => Permission::Upload,
            Self::Download { .. } => Permission::Download,
            _ => Permission::Act,
        }
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Info => "browser_info",
            Self::Snapshot => "browser_snapshot",
            Self::Navigate { .. } => "browser_navigate",
            Self::Upload { .. } => "browser_upload",
            Self::Download { .. } => "browser_download",
            _ => "browser_action",
        }
    }
    pub fn validate(&self) -> Result<(), AgentError> {
        match self {
            Self::Navigate { url } => validate_url(url)?,
            Self::Download {
                document,
                url,
                path,
            } => {
                document.validate()?;
                validate_url(url)?;
                if !transfer_path(path) {
                    return Err(AgentError::Configuration(
                        "bounded relative download path required",
                    ));
                }
            }
            Self::Info | Self::Snapshot => {}
            Self::Click { target } | Self::ScrollIntoView { target } => target.validate()?,
            Self::Fill { target, text } => {
                target.validate()?;
                if text.len() > 16 * 1024 || text.contains('\0') {
                    return Err(AgentError::Configuration("fill text limit"));
                }
            }
            Self::Select { target, values } => {
                target.validate()?;
                if values.len() > 16
                    || values.iter().any(|v| v.len() > 1024 || v.contains('\0'))
                    || values
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        != values.len()
                {
                    return Err(AgentError::Configuration(
                        "bounded unique selection values required",
                    ));
                }
            }
            Self::Press { target, key } => {
                target.validate()?;
                if press_key(key).is_none() {
                    return Err(AgentError::Configuration("unsupported browser key"));
                }
            }
            Self::Upload { target, path } => {
                target.validate()?;
                if !transfer_path(path) {
                    return Err(AgentError::Configuration(
                        "bounded relative upload path required",
                    ));
                }
            }
        }
        Ok(())
    }
    pub fn readonly(&self) -> bool {
        self.permission() == Permission::Inspect
    }
    pub fn accepts(&self, reply: &Reply) -> bool {
        match (self, reply) {
            (Self::Info, Reply::Info { .. })
            | (Self::Snapshot, Reply::Snapshot { .. })
            | (Self::Navigate { .. }, Reply::Navigated { .. }) => true,
            (Self::Download { path, .. }, Reply::Downloaded { path: actual, .. }) => path == actual,
            (_, Reply::Applied) => {
                matches!(self.permission(), Permission::Act | Permission::Upload)
            }
            _ => false,
        }
    }
}
pub fn validate_url(value: &str) -> Result<(), AgentError> {
    let valid = value == "about:blank"
        || (value.len() <= 2048
            && !value.chars().any(char::is_control)
            && url::Url::parse(value).is_ok_and(|u| {
                matches!(u.scheme(), "http" | "https")
                    && u.host_str().is_some()
                    && u.username().is_empty()
                    && u.password().is_none()
            }));
    if valid {
        Ok(())
    } else {
        Err(AgentError::Configuration(
            "bounded http(s) URL without credentials, or about:blank required",
        ))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub url: String,
    pub title: String,
    pub frame_id: String,
    pub loader_id: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomNode {
    pub node_id: i64,
    pub backend_node_id: i64,
    pub parent_id: Option<i64>,
    pub node_type: u16,
    pub name: String,
    pub text: String,
    pub attributes: Vec<(String, String)>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AxNode {
    pub node_id: String,
    pub backend_node_id: Option<i64>,
    pub role: String,
    pub name: String,
    pub value: String,
    pub ignored: bool,
    pub children: Vec<String>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Reply {
    Applied,
    Downloaded {
        path: String,
        bytes: usize,
    },
    Info {
        page: Page,
    },
    Navigated {
        page: Page,
    },
    Snapshot {
        page: Page,
        dom: Vec<DomNode>,
        accessibility: Vec<AxNode>,
        truncated: bool,
    },
}
impl Reply {
    pub fn validate(&self) -> Result<(), AgentError> {
        let (page, dom, ax) = match self {
            Self::Applied => return Ok(()),
            Self::Downloaded { path, bytes } => {
                return if transfer_path(path) && *bytes <= crate::protocol::FILE_LIMIT {
                    Ok(())
                } else {
                    Err(AgentError::Protocol("invalid downloaded file metadata"))
                };
            }
            Self::Info { page } | Self::Navigated { page } => (page, &[][..], &[][..]),
            Self::Snapshot {
                page,
                dom,
                accessibility,
                ..
            } => (page, dom.as_slice(), accessibility.as_slice()),
        };
        let mut bytes = 0usize;
        let mut text = |s: &str, limit: usize| {
            bytes = bytes.saturating_add(s.len());
            s.len() <= limit
        };
        let valid = text(&page.url, 2048)
            && text(&page.title, 2048)
            && text(&page.frame_id, 128)
            && text(&page.loader_id, 128)
            && !page.frame_id.is_empty()
            && dom.len() <= MAX_NODES
            && ax.len() <= MAX_NODES
            && dom.iter().all(|n| {
                n.node_id > 0
                    && n.backend_node_id > 0
                    && n.parent_id.is_none_or(|p| p > 0)
                    && n.node_type <= 12
                    && n.attributes.len() <= 16
                    && text(&n.name, 256)
                    && text(&n.text, 1024)
                    && n.attributes
                        .iter()
                        .all(|(k, v)| text(k, 256) && text(v, 512))
            })
            && ax.iter().all(|n| {
                !n.node_id.is_empty()
                    && n.backend_node_id.is_none_or(|p| p > 0)
                    && n.children.len() <= MAX_NODES
                    && text(&n.node_id, 128)
                    && text(&n.role, 256)
                    && text(&n.name, 1024)
                    && text(&n.value, 1024)
                    && n.children.iter().all(|c| text(c, 128))
            })
            && bytes <= TEXT_BUDGET;
        if valid {
            Ok(())
        } else {
            Err(AgentError::Protocol("invalid browser response"))
        }
    }
}
impl<E: crate::Engine> crate::Computer<'_, E> {
    pub fn browser(&mut self, request: Request, ctx: &TurnContext) -> Result<Reply, AgentError> {
        ctx.remaining()?;
        if !self.snapshot().spec.browser {
            return Err(AgentError::Configuration(
                "browser must be explicitly configured",
            ));
        }
        request.validate()?;
        match self.execute(
            crate::protocol::GuestRequest::Browser {
                request,
                timeout_ms: ctx.remaining()?.as_millis().clamp(1, 30000) as u64,
            },
            ctx,
        )? {
            crate::protocol::GuestValue::Browser { value } => Ok(value),
            _ => Err(AgentError::Protocol("browser response required")),
        }
    }
}
