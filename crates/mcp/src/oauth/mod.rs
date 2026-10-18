//! Explicit OAuth authorization-code/PKCE. No automatic interactive login/replay.
mod callback;
mod net;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use namzu_core::{AgentError, TurnContext};
use namzu_session::{PrivateStore, now_ms};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid or unsupported MCP OAuth configuration/metadata")]
    Invalid,
    #[error("MCP OAuth network operation failed")]
    Network,
    #[error("MCP OAuth response limit exceeded")]
    Limit,
    #[error("MCP OAuth HTTP status {0}")]
    Status(u16),
    #[error("MCP OAuth login required; use explicit mcp auth login")]
    LoginRequired,
    #[error("MCP OAuth exchange uncertain; explicit login required, no retry")]
    Uncertain,
    #[error("MCP OAuth authorization denied")]
    Denied,
    #[error("MCP OAuth callback rejected")]
    Callback,
    #[error(transparent)]
    Context(#[from] AgentError),
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub issuer: String,
    pub client_id: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret_env: Option<String>,
}
impl Settings {
    pub fn validate(&self) -> Result<(), Error> {
        endpoint(&self.issuer)?;
        if self.client_id.is_empty()
            || self.client_id.len() > 1024
            || self.client_id.chars().any(char::is_control)
            || self.scopes.len() > 32
            || self.scopes.iter().any(|s| !scope(s))
            || self.scopes.iter().collect::<BTreeSet<_>>().len() != self.scopes.len()
            || self.client_secret_env.as_ref().is_some_and(|s| {
                s.is_empty()
                    || s.len() > 128
                    || !s.bytes().enumerate().all(|(i, b)| {
                        b == b'_' || b.is_ascii_alphabetic() || i > 0 && b.is_ascii_digit()
                    })
            })
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
fn scope(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 256
        && s.bytes()
            .all(|b| b == 0x21 || (0x23..=0x5b).contains(&b) || (0x5d..=0x7e).contains(&b))
}
fn endpoint(s: &str) -> Result<Url, Error> {
    let u = crate::http::validate_endpoint(s).map_err(|_| Error::Invalid)?;
    if u.query().is_some() || s.len() > 4096 {
        return Err(Error::Invalid);
    }
    Ok(u)
}
fn random() -> Result<String, Error> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| Error::Invalid)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
/// Bound to exact resource URL and static client/issuer configuration; no Debug.
#[derive(Clone)]
pub struct Target {
    resource: String,
    settings: Settings,
    binding: String,
}
impl Target {
    pub fn new(resource: String, settings: Settings) -> Result<Self, Error> {
        endpoint(&resource)?;
        settings.validate()?;
        let binding = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(&resource, &settings)).map_err(|_| Error::Invalid)?
            )
        );
        Ok(Self {
            resource,
            settings,
            binding,
        })
    }
    pub(crate) fn from_server(server: &crate::config::Server) -> Result<Self, Error> {
        let crate::config::Transport::Http {
            url,
            bearer_env: None,
        } = &server.transport
        else {
            return Err(Error::Invalid);
        };
        Self::new(url.clone(), server.oauth.clone().ok_or(Error::Invalid)?)
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Metadata {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    require_iss: bool,
}
impl Metadata {
    fn validate(&self, target: &Target) -> Result<(), Error> {
        if self.issuer != target.settings.issuer {
            return Err(Error::Invalid);
        }
        let origin = endpoint(&self.issuer)?.origin();
        for s in [&self.authorization_endpoint, &self.token_endpoint] {
            if endpoint(s)?.origin() != origin {
                return Err(Error::Invalid);
            }
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Credential {
    version: u32,
    binding: String,
    metadata: Metadata,
    access_token: String,
    refresh_token: Option<String>,
    expires_ms: u64,
    scopes: Vec<String>,
    uncertain: bool,
}
impl Credential {
    fn validate(&self, t: &Target) -> Result<(), Error> {
        if self.version != 1
            || self.binding != t.binding
            || !token(&self.access_token)
            || self.refresh_token.as_ref().is_some_and(|s| !token(s))
            || self.expires_ms == 0
            || self.scopes.len() > 32
            || self
                .scopes
                .iter()
                .any(|s| !scope(s) || !t.settings.scopes.contains(s))
            || self.scopes.iter().collect::<BTreeSet<_>>().len() != self.scopes.len()
        {
            return Err(Error::Invalid);
        }
        self.metadata.validate(t)
    }
}
fn token(s: &str) -> bool {
    !s.is_empty() && s.len() <= 16384 && s.bytes().all(|b| b > 32 && b < 127)
}
#[derive(Serialize)]
pub struct Status {
    pub logged_in: bool,
    pub refresh_available: bool,
    pub uncertain: bool,
    pub expires_ms: Option<u64>,
}
pub struct Vault {
    store: PrivateStore,
}
impl Vault {
    pub fn new(store: PrivateStore) -> Self {
        Self { store }
    }
    fn name(t: &Target) -> String {
        format!("{}.json", t.binding)
    }
    fn read(&self, t: &Target) -> Result<Option<Credential>, Error> {
        let c: Option<Credential> = self.store.read(&Self::name(t))?;
        if let Some(c) = &c {
            c.validate(t)?;
        }
        Ok(c)
    }
    pub fn status(&self, t: &Target) -> Result<Status, Error> {
        let _lock = self.store.lock(&t.binding)?;
        let c = self.read(t)?;
        Ok(Status {
            logged_in: c
                .as_ref()
                .is_some_and(|c| !c.uncertain && c.expires_ms > now_ms() + 30000),
            refresh_available: c.as_ref().is_some_and(|c| c.refresh_token.is_some()),
            uncertain: c.as_ref().is_some_and(|c| c.uncertain),
            expires_ms: c.map(|c| c.expires_ms),
        })
    }
    /// Local deletion only. Does not claim remote provider token revocation.
    pub fn logout(&self, t: &Target) -> Result<(), Error> {
        let _lock = self.store.lock(&t.binding)?;
        self.store.remove(&Self::name(t))?;
        Ok(())
    }
    pub fn login(
        &self,
        t: &Target,
        port: u16,
        ctx: &TurnContext,
        display: impl FnOnce(&str),
    ) -> Result<(), Error> {
        ctx.remaining()?;
        let _lock = self.store.lock(&t.binding)?;
        let net = net::Net::new()?;
        let metadata = net.discover(t, ctx)?;
        let callback = callback::Listener::bind(port)?;
        let redirect = callback.redirect();
        let verifier = random()?;
        let state = random()?;
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut url = endpoint(&metadata.authorization_endpoint)?;
        url.query_pairs_mut().extend_pairs([
            ("response_type", "code"),
            ("client_id", &t.settings.client_id),
            ("redirect_uri", &redirect),
            ("state", &state),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("resource", &t.resource),
        ]);
        if !t.settings.scopes.is_empty() {
            url.query_pairs_mut()
                .append_pair("scope", &t.settings.scopes.join(" "));
        }
        display(url.as_str());
        let code = callback.wait(&state, &metadata, ctx)?;
        // Replace the old credential before an uncertain code exchange. Never
        // leave an old identity silently active after a failed explicit login.
        self.store.remove(&Self::name(t))?;
        let form = vec![
            ("grant_type".into(), "authorization_code".into()),
            ("code".into(), code),
            ("redirect_uri".into(), redirect),
            ("code_verifier".into(), verifier),
        ];
        let credential = net.exchange(t, &metadata, form, None, ctx)?;
        self.store.write(&Self::name(t), &credential)?;
        Ok(())
    }
    /// Refresh only before a new HTTP connection. No RPC/session retry.
    pub fn access_token(&self, t: &Target, ctx: &TurnContext) -> Result<String, Error> {
        ctx.remaining()?;
        let _lock = self.store.lock(&t.binding)?;
        let mut c = self.read(t)?.ok_or(Error::LoginRequired)?;
        if c.uncertain {
            return Err(Error::Uncertain);
        }
        if c.expires_ms > now_ms() + 30000 {
            return Ok(c.access_token);
        }
        let refresh = c.refresh_token.clone().ok_or(Error::LoginRequired)?;
        let net = net::Net::new()?;
        // Mark before sending a rotating grant; timeout/crash cannot replay it.
        c.uncertain = true;
        self.store.write(&Self::name(t), &c)?;
        let next = net.exchange(
            t,
            &c.metadata,
            vec![
                ("grant_type".into(), "refresh_token".into()),
                ("refresh_token".into(), refresh),
            ],
            Some(&c),
            ctx,
        )?;
        self.store.write(&Self::name(t), &next)?;
        Ok(next.access_token)
    }
}
