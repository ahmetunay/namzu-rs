//! Private, atomic state storage with fail-closed interrupted-turn recovery.
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use namzu_core::{AgentError, Message, Observer, Stats, ToolCall};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_STATE: usize = 16 * 1024 * 1024;
static SERIAL: AtomicU64 = AtomicU64::new(0);
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
pub fn new_id() -> String {
    format!(
        "s-{}-{}-{}",
        now_ms(),
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    )
}
fn name_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        && name != "."
        && name != ".."
}

pub fn default_state_dir() -> Result<PathBuf, AgentError> {
    if let Some(path) = std::env::var_os("NAMZU_STATE_DIR") {
        return Ok(path.into());
    }
    if let Some(path) = std::env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(path).join("namzu-rs"));
    }
    Ok(PathBuf::from(
        std::env::var_os("HOME").ok_or(AgentError::State("state directory selector required"))?,
    )
    .join(".local/state/namzu-rs"))
}

pub struct PrivateStore {
    dir: Dir,
}
impl PrivateStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AgentError> {
        let path = path.as_ref();
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(path)
            .map_err(|_| AgentError::State("cannot create private state directory"))?;
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|_| AgentError::State("cannot inspect state directory"))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(AgentError::State("state root must be a real directory"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
                return Err(AgentError::State(
                    "state directory must be owned by current user and mode 0700",
                ));
            }
        }
        Ok(Self {
            dir: Dir::open_ambient_dir(path, ambient_authority())
                .map_err(|_| AgentError::State("cannot open state directory"))?,
        })
    }
    fn options() -> OpenOptions {
        let mut options = OpenOptions::new();
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .mode(0o600);
        }
        options
    }
    pub fn lock(&self, name: &str) -> Result<StoreLock, AgentError> {
        if !name_valid(name) {
            return Err(AgentError::State("invalid lock name"));
        }
        let mut options = Self::options();
        options.read(true).write(true).create(true);
        let file = self
            .dir
            .open_with(format!("{name}.lock"), &options)
            .map_err(|_| AgentError::State("cannot open state lock"))?
            .into_std();
        check_file(&file)?;
        fs2::FileExt::try_lock_exclusive(&file)
            .map_err(|_| AgentError::State("state is busy in another process"))?;
        Ok(StoreLock(file))
    }
    pub fn read<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>, AgentError> {
        if !name_valid(name) {
            return Err(AgentError::State("invalid state name"));
        }
        let mut options = Self::options();
        options.read(true);
        let file = match self.dir.open_with(name, &options) {
            Ok(file) => file.into_std(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(AgentError::State("cannot open state file")),
        };
        check_file(&file)?;
        let mut bytes = vec![];
        file.take((MAX_STATE + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| AgentError::State("cannot read state file"))?;
        if bytes.len() > MAX_STATE {
            return Err(AgentError::State("state file exceeds size limit"));
        }
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| AgentError::State("invalid state JSON"))
    }
    pub fn write<T: Serialize>(&self, name: &str, value: &T) -> Result<(), AgentError> {
        if !name_valid(name) {
            return Err(AgentError::State("invalid state name"));
        }
        let bytes =
            serde_json::to_vec(value).map_err(|_| AgentError::State("cannot encode state"))?;
        if bytes.len() > MAX_STATE {
            return Err(AgentError::State("state exceeds size limit"));
        }
        let temporary = format!(
            ".tmp-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        );
        let mut options = Self::options();
        options.write(true).create_new(true);
        let mut file = self
            .dir
            .open_with(&temporary, &options)
            .map_err(|_| AgentError::State("cannot create private temporary state"))?;
        let result = (|| {
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|_| AgentError::State("cannot sync state"))?;
            self.dir
                .rename(&temporary, &self.dir, name)
                .map_err(|_| AgentError::State("cannot publish state"))?;
            self.dir
                .open(".")
                .map_err(|_| AgentError::State("cannot open directory for sync"))?
                .sync_all()
                .map_err(|_| AgentError::State("cannot sync state directory"))
        })();
        if result.is_err() {
            let _ = self.dir.remove_file(&temporary);
        }
        result
    }
    pub fn remove(&self, name: &str) -> Result<(), AgentError> {
        if !name_valid(name) {
            return Err(AgentError::State("invalid state name"));
        }
        match self.dir.remove_file(name) {
            Ok(()) => self
                .dir
                .open(".")
                .and_then(|directory| directory.sync_all())
                .map_err(|_| AgentError::State("cannot sync state removal")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(AgentError::State("cannot remove state file")),
        }
    }
    pub fn names(&self) -> Result<Vec<String>, AgentError> {
        let mut names = vec![];
        for entry in self
            .dir
            .entries()
            .map_err(|_| AgentError::State("cannot list state"))?
        {
            if names.len() >= 10000 {
                return Err(AgentError::State("too many state files"));
            }
            if let Some(name) = entry
                .map_err(|_| AgentError::State("cannot list state entry"))?
                .file_name()
                .to_str()
            {
                names.push(name.into());
            }
        }
        names.sort();
        Ok(names)
    }
}
fn check_file(file: &std::fs::File) -> Result<(), AgentError> {
    let metadata = file
        .metadata()
        .map_err(|_| AgentError::State("cannot inspect state file"))?;
    if !metadata.is_file() {
        return Err(AgentError::State("state must be a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
            || metadata.nlink() != 1
        {
            return Err(AgentError::State(
                "state file must be private, owned, and not hardlinked",
            ));
        }
    }
    Ok(())
}
pub struct StoreLock(std::fs::File);
impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Running,
    Complete,
    Failed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub version: u32,
    pub id: String,
    pub cwd: PathBuf,
    pub route: String,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub status: SessionStatus,
    pub messages: Vec<Message>,
    pub stats: Stats,
    pub pending_call: Option<String>,
}
impl Session {
    pub fn new(id: String, cwd: PathBuf, route: String) -> Result<Self, AgentError> {
        if !name_valid(&id) {
            return Err(AgentError::State("invalid session ID"));
        }
        Ok(Self {
            version: 1,
            id,
            cwd,
            route,
            created_ms: now_ms(),
            updated_ms: now_ms(),
            status: SessionStatus::Complete,
            messages: vec![],
            stats: Stats {
                provider_calls: 0,
                tool_calls: 0,
            },
            pending_call: None,
        })
    }
    pub fn load(store: &PrivateStore, id: &str) -> Result<Self, AgentError> {
        if !name_valid(id) {
            return Err(AgentError::State("invalid session ID"));
        }
        let session: Self = store
            .read(&format!("{id}.json"))?
            .ok_or(AgentError::State("session not found"))?;
        if session.version != 1 || session.id != id {
            return Err(AgentError::State("invalid session version or identity"));
        }
        Ok(session)
    }
    pub fn save(&self, store: &PrivateStore) -> Result<(), AgentError> {
        store.write(&format!("{}.json", self.id), self)
    }
    pub fn require_resumable(&self, cwd: &Path, route: &str) -> Result<(), AgentError> {
        if self.status != SessionStatus::Complete || self.pending_call.is_some() {
            return Err(AgentError::State(
                "interrupted session: inspect effects; automatic replay refused; start a new session",
            ));
        }
        if self.cwd != cwd || self.route != route {
            return Err(AgentError::State(
                "resume requires the original workspace and provider/model route",
            ));
        }
        Ok(())
    }
}
pub struct Journal<'a> {
    pub store: &'a PrivateStore,
    pub session: &'a mut Session,
}
impl Observer for Journal<'_> {
    fn checkpoint(
        &mut self,
        messages: &[Message],
        stats: &Stats,
        pending_call: Option<&ToolCall>,
    ) -> Result<(), AgentError> {
        self.session.messages = messages.to_vec();
        self.session.stats = stats.clone();
        self.session.pending_call = pending_call.map(|call| call.id.clone());
        self.session.updated_ms = now_ms();
        self.session.status = SessionStatus::Running;
        self.session.save(self.store)
    }
}
