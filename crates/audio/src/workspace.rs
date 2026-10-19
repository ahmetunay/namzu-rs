use crate::{MAX_AUDIO_BYTES, Wav};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use namzu_core::{AgentError, TurnContext};
use std::{
    io::{Read, Write},
    path::{Component, Path},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
pub struct AudioWorkspace {
    root: Dir,
}
fn error() -> AgentError {
    AgentError::State("cannot access private workspace audio file")
}
fn path_ok(path: &str) -> Result<(), AgentError> {
    if path.is_empty()
        || path.len() > 4096
        || path.contains('\0')
        || Path::new(path).components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(AgentError::Configuration(
            "relative workspace audio path required",
        ));
    }
    Ok(())
}
impl AudioWorkspace {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, AgentError> {
        Ok(Self {
            root: Dir::open_ambient_dir(root, ambient_authority()).map_err(|_| error())?,
        })
    }
    pub fn read(&self, path: &str, ctx: &TurnContext) -> Result<Wav, AgentError> {
        path_ok(path)?;
        ctx.remaining()?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let mut file = self.root.open_with(path, &options).map_err(|_| error())?;
        let metadata = file.metadata().map_err(|_| error())?;
        if !metadata.is_file() || metadata.len() > MAX_AUDIO_BYTES as u64 {
            return Err(error());
        }
        let mut bytes = Vec::new();
        let mut block = [0; 8192];
        loop {
            ctx.remaining()?;
            let n = file.read(&mut block).map_err(|_| error())?;
            if n == 0 {
                break;
            }
            if bytes.len() + n > MAX_AUDIO_BYTES {
                return Err(error());
            }
            bytes.extend_from_slice(&block[..n]);
        }
        ctx.remaining()?;
        let audio = Wav::parse(bytes)?;
        ctx.remaining()?;
        Ok(audio)
    }
    fn parent(&self, path: &str) -> Result<(Dir, std::ffi::OsString), AgentError> {
        path_ok(path)?;
        let path = Path::new(path);
        let name = path
            .file_name()
            .ok_or(AgentError::Configuration("audio output filename required"))?
            .to_os_string();
        let parent = self
            .root
            .open_dir(
                path.parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
            )
            .map_err(|_| error())?;
        Ok((parent, name))
    }
    pub fn check_destination(&self, path: &str) -> Result<(), AgentError> {
        let (parent, name) = self.parent(path)?;
        match parent.symlink_metadata(&name) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(AgentError::State(
                "audio destination exists or is inaccessible",
            )),
        }
    }
    pub fn publish(&self, path: &str, audio: &Wav, ctx: &TurnContext) -> Result<(), AgentError> {
        ctx.remaining()?;
        self.check_destination(path)?;
        let (parent, name) = self.parent(path)?;
        let temp = format!(
            ".namzu-audio-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = parent.open_with(&temp, &options).map_err(|_| error())?;
        let result = (|| {
            for chunk in audio.bytes().chunks(8192) {
                ctx.remaining()?;
                file.write_all(chunk).map_err(|_| error())?;
            }
            file.sync_all().map_err(|_| error())?;
            ctx.remaining()?;
            parent
                .hard_link(&temp, &parent, &name)
                .map_err(|_| error())?;
            Ok(())
        })();
        drop(file);
        let removed = parent.remove_file(&temp).map_err(|_| error());
        result?;
        removed?;
        Ok(())
    }
}
