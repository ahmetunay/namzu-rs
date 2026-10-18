//! Private profiles, explicit project snapshots and fail-closed layered admission.
use namzu_core::AgentError;
use namzu_session::{PrivateStore, StoreLock};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid MCP configuration")]
    Configuration,
    #[error("MCP profile or server not found")]
    NotFound,
    #[error("MCP project configuration requires trust for its current content")]
    TrustRequired,
    #[error("MCP operation denied by policy")]
    Denied,
    #[error("MCP operation requires explicit one-operation approval")]
    ApprovalRequired,
    #[error("MCP permission belongs to a stale configuration or another manager")]
    Stale,
    #[error("MCP server is not connected")]
    NotConnected,
    #[error(
        "MCP server has uncertain effects; inspect abandoned requests before a new manager generation"
    )]
    Uncertain,
    #[error("MCP server is already connected")]
    Connected,
    #[error(transparent)]
    State(#[from] AgentError),
    #[error(transparent)]
    Protocol(#[from] crate::Error),
    #[cfg(unix)]
    #[error(transparent)]
    Stdio(#[from] crate::stdio::Error),
    #[error(transparent)]
    Http(#[from] crate::http::Error),
    #[error(transparent)]
    Discovery(#[from] crate::discovery::Error),
    #[error(transparent)]
    OAuth(#[from] crate::oauth::Error),
    #[error("MCP transport is unavailable on this platform")]
    Unsupported,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    Ask,
    Deny,
}
fn yes() -> bool {
    true
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    pub allow: Option<BTreeSet<String>>,
    pub deny: BTreeSet<String>,
    pub require_approval: bool,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            allow: None,
            deny: BTreeSet::new(),
            require_approval: true,
        }
    }
}
impl Policy {
    fn validate(&self) -> Result<(), Error> {
        for set in [self.allow.as_ref(), Some(&self.deny)]
            .into_iter()
            .flatten()
        {
            if set.len() > 256
                || set.iter().any(|s| {
                    s.is_empty()
                        || s.len() > 4096
                        || s.chars().any(char::is_control)
                        || s.contains('*')
                            && s != "*"
                            && !(s.ends_with(":*") && s.matches('*').count() == 1)
                })
            {
                return Err(Error::Configuration);
            }
        }
        Ok(())
    }
    pub fn decide(&self, target: &str) -> Decision {
        let matches = |s: &String| {
            s == "*"
                || s == target
                || s == target.split(':').next().unwrap_or(target)
                || s.strip_suffix(":*")
                    .is_some_and(|prefix| target.starts_with(&format!("{prefix}:")))
        };
        if self.deny.iter().any(matches)
            || self
                .allow
                .as_ref()
                .is_some_and(|allow| !allow.iter().any(matches))
        {
            Decision::Deny
        } else if self.require_approval {
            Decision::Ask
        } else {
            Decision::Allow
        }
    }
}
/// Config stores environment variable selectors, never a literal bearer/env value.
/// No Debug: literal argv and URLs may contain private data.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Transport {
    Stdio {
        program: PathBuf,
        #[serde(default)]
        args: Vec<String>,
        cwd: PathBuf,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    Http {
        url: String,
        #[serde(default)]
        bearer_env: Option<String>,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Server {
    #[serde(default = "yes")]
    pub enabled: bool,
    pub transport: Transport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<crate::oauth::Settings>,
    #[serde(default)]
    pub policy: Policy,
    #[serde(default = "timeout")]
    pub timeout_secs: u64,
}
fn timeout() -> u64 {
    300
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub servers: BTreeMap<String, Server>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub version: u32,
    pub default_profile: String,
    pub profiles: BTreeMap<String, Profile>,
}
impl Default for Document {
    fn default() -> Self {
        Self {
            version: 1,
            default_profile: "default".into(),
            profiles: BTreeMap::from([("default".into(), Profile::default())]),
        }
    }
}
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn env_name(name: &str) -> bool {
    name.len() <= 128
        && name
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
impl Document {
    pub fn validate(&self) -> Result<(), Error> {
        if self.version != 1
            || self.profiles.is_empty()
            || self.profiles.len() > 32
            || !self.profiles.contains_key(&self.default_profile)
        {
            return Err(Error::Configuration);
        }
        for (name, profile) in &self.profiles {
            if !valid_name(name) || profile.servers.len() > 64 {
                return Err(Error::Configuration);
            }
            for (name, server) in &profile.servers {
                if !valid_name(name) || !(1..=300).contains(&server.timeout_secs) {
                    return Err(Error::Configuration);
                }
                server.policy.validate()?;
                if server.oauth.is_some() {
                    crate::oauth::Target::from_server(server).map_err(|_| Error::Configuration)?;
                }
                match &server.transport {
                    Transport::Stdio {
                        program,
                        args,
                        cwd,
                        env,
                    } => {
                        if !program.is_absolute()
                            || !cwd.is_absolute()
                            || args.len() > 128
                            || args.iter().map(String::len).sum::<usize>() > 65536
                            || args.iter().any(|s| s.contains('\0'))
                            || env.len() > 64
                            || env.iter().any(|(k, v)| !env_name(k) || !env_name(v))
                        {
                            return Err(Error::Configuration);
                        }
                    }
                    Transport::Http { url, bearer_env } => {
                        crate::http::validate_endpoint(url).map_err(|_| Error::Configuration)?;
                        if bearer_env.as_ref().is_some_and(|v| !env_name(v)) {
                            return Err(Error::Configuration);
                        }
                    }
                }
            }
        }
        if serde_json::to_vec(self)
            .map_err(|_| Error::Configuration)?
            .len()
            > 256 * 1024
        {
            return Err(Error::Configuration);
        }
        Ok(())
    }
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > 256 * 1024 {
            return Err(Error::Configuration);
        }
        // Reuse strict duplicate-key/depth/node validation before the typed schema.
        crate::wire::validate_config_json(bytes)?;
        let doc: Self = serde_json::from_slice(bytes).map_err(|_| Error::Configuration)?;
        doc.validate()?;
        Ok(doc)
    }
}
pub fn read_document(path: &Path) -> Result<Document, Error> {
    Document::parse(&read_file(path)?)
}
pub fn read_server(path: &Path) -> Result<Server, Error> {
    let bytes = read_file(path)?;
    crate::wire::validate_config_json(&bytes)?;
    let server: Server = serde_json::from_slice(&bytes).map_err(|_| Error::Configuration)?;
    let mut doc = Document::default();
    doc.profiles
        .get_mut("default")
        .expect("default profile")
        .servers
        .insert("validate".into(), server.clone());
    doc.validate()?;
    Ok(server)
}
fn read_file(path: &Path) -> Result<Vec<u8>, Error> {
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = opts.open(path).map_err(|_| Error::Configuration)?;
    bounded_file(file)
}
fn bounded_file(file: std::fs::File) -> Result<Vec<u8>, Error> {
    if !file.metadata().map_err(|_| Error::Configuration)?.is_file() {
        return Err(Error::Configuration);
    }
    let mut bytes = vec![];
    file.take(256 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Configuration)?;
    if bytes.len() > 256 * 1024 {
        return Err(Error::Configuration);
    }
    Ok(bytes)
}
/// Config path must be explicitly selected and relative inside a canonical project.
#[derive(Clone)]
pub struct Project {
    root: PathBuf,
    config: PathBuf,
}
impl Project {
    pub fn new(root: &Path, config: &Path) -> Result<Self, Error> {
        if config.as_os_str().is_empty()
            || config
                .components()
                .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
        {
            return Err(Error::Configuration);
        }
        let root = root.canonicalize().map_err(|_| Error::Configuration)?;
        if !root.is_dir() {
            return Err(Error::Configuration);
        }
        Ok(Self {
            root,
            config: config.into(),
        })
    }
    fn load(&self) -> Result<(Document, String), Error> {
        let dir = cap_std::fs::Dir::open_ambient_dir(&self.root, cap_std::ambient_authority())
            .map_err(|_| Error::Configuration)?;
        let mut opts = cap_std::fs::OpenOptions::new();
        opts.read(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = dir
            .open_with(&self.config, &opts)
            .map_err(|_| Error::Configuration)?
            .into_std();
        let bytes = bounded_file(file)?;
        let doc = Document::parse(&bytes)?;
        let key = hash(&(&self.root, &self.config, bytes))?;
        Ok((doc, key))
    }
}
fn hash(value: &impl Serialize) -> Result<String, Error> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).map_err(|_| Error::Configuration)?)
    ))
}
#[derive(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Trust {
    projects: BTreeMap<PathBuf, String>,
}
pub struct OAuthLogin<'a> {
    pub profile: Option<&'a str>,
    pub project: Option<&'a Project>,
    pub server: &'a str,
    pub ceiling: Policy,
    pub approve: bool,
    pub port: u16,
}
pub struct Registry {
    store: PrivateStore,
}
impl Registry {
    pub fn open(state: &Path) -> Result<Self, Error> {
        Ok(Self {
            store: PrivateStore::open(state)?.child("mcp")?,
        })
    }
    pub(crate) fn oauth_store(&self) -> Result<PrivateStore, Error> {
        Ok(self.store.child("oauth")?)
    }
    /// Explicit host operation. Hold configuration admission during the login;
    /// no agent or HTTP challenge starts this flow automatically.
    pub fn oauth_login(
        &self,
        options: OAuthLogin<'_>,
        ctx: &namzu_core::TurnContext,
        display: impl FnOnce(&str),
    ) -> Result<(), Error> {
        let _lock = self.lock()?;
        let resolved = self.resolve(options.profile, options.project, &options.ceiling)?;
        let entry = resolved
            .servers
            .get(options.server)
            .ok_or(Error::NotFound)?;
        match entry.decide("oauth/login") {
            Decision::Deny => return Err(Error::Denied),
            Decision::Ask if !options.approve => return Err(Error::ApprovalRequired),
            _ => {}
        }
        let target = crate::oauth::Target::from_server(&entry.server)?;
        crate::oauth::Vault::new(self.oauth_store()?).login(&target, options.port, ctx, display)?;
        Ok(())
    }
    pub fn oauth_status(
        &self,
        profile: Option<&str>,
        project: Option<&Project>,
        server: &str,
        logout: bool,
    ) -> Result<crate::oauth::Status, Error> {
        let _lock = self.lock()?;
        let resolved = self.resolve(
            profile,
            project,
            &Policy {
                require_approval: false,
                ..Policy::default()
            },
        )?;
        let entry = resolved.servers.get(server).ok_or(Error::NotFound)?;
        let target = crate::oauth::Target::from_server(&entry.server)?;
        let vault = crate::oauth::Vault::new(self.oauth_store()?);
        if logout {
            vault.logout(&target)?;
        }
        Ok(vault.status(&target)?)
    }
    pub(crate) fn lock(&self) -> Result<StoreLock, Error> {
        Ok(self.store.lock("registry")?)
    }
    fn document(&self) -> Result<Document, Error> {
        let doc: Document = self.store.read("profiles.json")?.unwrap_or_default();
        doc.validate()?;
        Ok(doc)
    }
    pub fn init(&self) -> Result<(), Error> {
        let _lock = self.lock()?;
        Ok(self
            .store
            .write_new("profiles.json", &Document::default())?)
    }
    pub fn replace(&self, doc: Document) -> Result<(), Error> {
        doc.validate()?;
        let _lock = self.lock()?;
        Ok(self.store.write("profiles.json", &doc)?)
    }
    pub fn profiles(&self) -> Result<(String, Vec<String>), Error> {
        let _lock = self.lock()?;
        let doc = self.document()?;
        Ok((doc.default_profile, doc.profiles.into_keys().collect()))
    }
    pub fn create_profile(&self, name: &str) -> Result<(), Error> {
        let _lock = self.lock()?;
        let mut doc = self.document()?;
        if !valid_name(name) || doc.profiles.contains_key(name) {
            return Err(Error::Configuration);
        }
        doc.profiles.insert(name.into(), Profile::default());
        doc.validate()?;
        Ok(self.store.write("profiles.json", &doc)?)
    }
    pub fn default_profile(&self, name: &str) -> Result<(), Error> {
        let _lock = self.lock()?;
        let mut doc = self.document()?;
        if !doc.profiles.contains_key(name) {
            return Err(Error::NotFound);
        }
        doc.default_profile = name.into();
        Ok(self.store.write("profiles.json", &doc)?)
    }
    pub fn remove_profile(&self, name: &str) -> Result<(), Error> {
        let _lock = self.lock()?;
        let mut doc = self.document()?;
        if name == doc.default_profile {
            return Err(Error::Configuration);
        }
        doc.profiles.remove(name).ok_or(Error::NotFound)?;
        Ok(self.store.write("profiles.json", &doc)?)
    }
    pub fn put_server(
        &self,
        profile: Option<&str>,
        name: &str,
        server: Server,
        replace: bool,
    ) -> Result<(), Error> {
        if !valid_name(name) {
            return Err(Error::Configuration);
        }
        let _lock = self.lock()?;
        let mut doc = self.document()?;
        let selected = profile.unwrap_or(&doc.default_profile).to_owned();
        let servers = &mut doc
            .profiles
            .get_mut(&selected)
            .ok_or(Error::NotFound)?
            .servers;
        if servers.contains_key(name) != replace {
            return Err(Error::Configuration);
        }
        servers.insert(name.into(), server);
        doc.validate()?;
        Ok(self.store.write("profiles.json", &doc)?)
    }
    pub fn edit_server(
        &self,
        profile: Option<&str>,
        name: &str,
        enabled: Option<bool>,
    ) -> Result<(), Error> {
        let _lock = self.lock()?;
        let mut doc = self.document()?;
        let selected = profile.unwrap_or(&doc.default_profile).to_owned();
        let servers = &mut doc
            .profiles
            .get_mut(&selected)
            .ok_or(Error::NotFound)?
            .servers;
        if let Some(value) = enabled {
            servers.get_mut(name).ok_or(Error::NotFound)?.enabled = value;
        } else {
            servers.remove(name).ok_or(Error::NotFound)?;
        }
        Ok(self.store.write("profiles.json", &doc)?)
    }
    pub fn trust(&self, project: &Project) -> Result<String, Error> {
        let _lock = self.lock()?;
        let (_, digest) = project.load()?;
        let mut trust: Trust = self.store.read("trust.json")?.unwrap_or_default();
        if trust.projects.len() >= 256 && !trust.projects.contains_key(&project.root) {
            return Err(Error::Configuration);
        }
        trust.projects.insert(project.root.clone(), digest.clone());
        self.store.write("trust.json", &trust)?;
        Ok(digest)
    }
    pub fn untrust(&self, project: &Project) -> Result<(), Error> {
        let _lock = self.lock()?;
        let mut trust: Trust = self.store.read("trust.json")?.unwrap_or_default();
        trust.projects.remove(&project.root);
        Ok(self.store.write("trust.json", &trust)?)
    }
    pub fn list(
        &self,
        profile: Option<&str>,
        project: Option<&Project>,
        ceiling: &Policy,
    ) -> Result<Vec<Summary>, Error> {
        let _lock = self.lock()?;
        Ok(self.resolve(profile, project, ceiling)?.summaries())
    }
    pub(crate) fn resolve(
        &self,
        profile: Option<&str>,
        project: Option<&Project>,
        ceiling: &Policy,
    ) -> Result<Resolved, Error> {
        ceiling.validate()?;
        let doc = self.document()?;
        let selected = profile.unwrap_or(&doc.default_profile);
        let global = doc.profiles.get(selected).ok_or(Error::NotFound)?;
        let mut servers = BTreeMap::new();
        for (name, server) in &global.servers {
            servers.insert(
                name.clone(),
                Entry {
                    server: server.clone(),
                    policies: vec![ceiling.clone(), server.policy.clone()],
                    project: false,
                },
            );
        }
        let mut project_hash = None;
        if let Some(project) = project {
            let (local, digest) = project.load()?;
            let trust: Trust = self.store.read("trust.json")?.unwrap_or_default();
            if trust.projects.get(&project.root) != Some(&digest) {
                return Err(Error::TrustRequired);
            }
            if let Some(local) = local.profiles.get(selected) {
                for (name, server) in &local.servers {
                    let mut entry = Entry {
                        server: server.clone(),
                        policies: vec![ceiling.clone(), server.policy.clone()],
                        project: true,
                    };
                    if let Some(global) = global.servers.get(name) {
                        entry.server.enabled &= global.enabled;
                        entry.policies.push(global.policy.clone());
                    }
                    servers.insert(name.clone(), entry);
                }
            }
            project_hash = Some(digest);
        }
        let fingerprint = hash(&(&doc, selected, project_hash, ceiling))?;
        Ok(Resolved {
            servers,
            fingerprint,
        })
    }
}
pub(crate) struct Entry {
    pub server: Server,
    pub policies: Vec<Policy>,
    pub project: bool,
}
impl Entry {
    pub fn decide(&self, target: &str) -> Decision {
        if !self.server.enabled {
            return Decision::Deny;
        }
        self.policies
            .iter()
            .map(|p| p.decide(target))
            .max()
            .unwrap_or(Decision::Deny)
    }
}
pub(crate) struct Resolved {
    pub servers: BTreeMap<String, Entry>,
    pub fingerprint: String,
}
#[derive(Serialize)]
pub struct Summary {
    pub name: String,
    pub enabled: bool,
    pub transport: &'static str,
    pub source: &'static str,
    pub connect: Decision,
}
impl Resolved {
    fn summaries(&self) -> Vec<Summary> {
        self.servers
            .iter()
            .map(|(name, entry)| Summary {
                name: name.clone(),
                enabled: entry.server.enabled,
                transport: match entry.server.transport {
                    Transport::Stdio { .. } => "stdio",
                    Transport::Http { .. } => "http",
                },
                source: if entry.project { "project" } else { "user" },
                connect: entry.decide("connect"),
            })
            .collect()
    }
}
