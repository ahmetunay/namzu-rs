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
    Navigate { url: String },
    Snapshot,
}
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BrowserRequest(<redacted>)")
    }
}
impl Request {
    pub fn validate(&self) -> Result<(), AgentError> {
        if let Self::Navigate { url } = self {
            validate_url(url)?;
        }
        Ok(())
    }
    pub fn readonly(&self) -> bool {
        !matches!(self, Self::Navigate { .. })
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
#[derive(Debug, Serialize, Deserialize)]
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
