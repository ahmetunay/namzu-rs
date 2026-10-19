use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use namzu_core::{AgentError, TurnContext};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use url::Url;
pub const DOCUMENT_BYTES: usize = 1024 * 1024;
static NEXT: AtomicU64 = AtomicU64::new(0);
pub(crate) fn bad(s: &'static str) -> AgentError {
    AgentError::Protocol(s)
}
pub fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextEdit {
    pub range: Range,
    pub new_text: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EditPlan {
    pub uri: String,
    pub version: i32,
    pub expected_sha256: String,
    pub edits: Vec<TextEdit>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Applied {
    pub uri: String,
    pub previous_sha256: String,
    pub sha256: String,
    pub bytes: usize,
}
#[derive(Clone)]
pub struct Document {
    pub uri: String,
    pub version: i32,
    pub sha256: String,
    pub text: String,
    pub language: String,
}
impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Document")
            .field("uri", &self.uri)
            .field("version", &self.version)
            .field("sha256", &self.sha256)
            .finish()
    }
}
pub struct Workspace {
    pub(crate) root: PathBuf,
    dir: Dir,
}
fn path_ok(path: &str) -> Result<(), AgentError> {
    if path.is_empty()
        || path.len() > 4096
        || path.contains('\0')
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(bad("relative LSP workspace path required"));
    }
    Ok(())
}
impl Workspace {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, AgentError> {
        let root = root
            .as_ref()
            .canonicalize()
            .map_err(|_| bad("cannot open LSP root"))?;
        let dir = Dir::open_ambient_dir(&root, ambient_authority())
            .map_err(|_| bad("cannot open LSP root"))?;
        Ok(Self { root, dir })
    }
    pub fn root_uri(&self) -> String {
        Url::from_directory_path(&self.root).unwrap().to_string()
    }
    pub fn uri(&self, path: &str) -> Result<String, AgentError> {
        path_ok(path)?;
        Url::from_file_path(self.root.join(path))
            .map(String::from)
            .map_err(|_| bad("cannot encode LSP file URI"))
    }
    pub(crate) fn relative(&self, uri: &str) -> Result<PathBuf, AgentError> {
        let url = Url::parse(uri).map_err(|_| bad("invalid LSP document URI"))?;
        if url.scheme() != "file"
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.host_str().is_some_and(|h| h != "localhost")
        {
            return Err(bad("local workspace file URI required"));
        }
        let path = url
            .to_file_path()
            .map_err(|_| bad("invalid LSP file URI"))?;
        let relative = path
            .strip_prefix(&self.root)
            .map_err(|_| bad("LSP URI outside workspace"))?;
        path_ok(relative.to_str().ok_or(bad("UTF-8 LSP path required"))?)?;
        Ok(relative.to_path_buf())
    }
    pub fn read(&self, path: &str, ctx: &TurnContext) -> Result<String, AgentError> {
        path_ok(path)?;
        self.read_in(&self.dir, Path::new(path), ctx)
    }
    fn read_in(&self, dir: &Dir, path: &Path, ctx: &TurnContext) -> Result<String, AgentError> {
        ctx.remaining()?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let mut file = dir
            .open_with(path, &options)
            .map_err(|_| bad("cannot read LSP workspace file"))?;
        let meta = file
            .metadata()
            .map_err(|_| bad("cannot inspect LSP file"))?;
        if !meta.is_file() || meta.len() > DOCUMENT_BYTES as u64 {
            return Err(bad("bounded regular LSP file required"));
        }
        let mut bytes = Vec::new();
        let mut block = [0; 8192];
        loop {
            ctx.remaining()?;
            let n = file
                .read(&mut block)
                .map_err(|_| bad("cannot read LSP file"))?;
            if n == 0 {
                break;
            }
            if bytes.len() + n > DOCUMENT_BYTES {
                return Err(bad("LSP file grew beyond limit"));
            }
            bytes.extend_from_slice(&block[..n]);
        }
        ctx.remaining()?;
        String::from_utf8(bytes).map_err(|_| bad("LSP document requires UTF-8"))
    }
    pub fn load_plan(&self, path: &str, ctx: &TurnContext) -> Result<EditPlan, AgentError> {
        serde_json::from_str(&self.read(path, ctx)?).map_err(|_| bad("invalid LSP edit plan"))
    }
    pub fn preview(&self, plan: &EditPlan, ctx: &TurnContext) -> Result<String, AgentError> {
        if plan.version < 1
            || plan.expected_sha256.len() != 64
            || !plan
                .expected_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(bad("invalid version or hash in edit plan"));
        }
        let path = self.relative(&plan.uri)?;
        let current = self.read(path.to_str().ok_or(bad("UTF-8 LSP path required"))?, ctx)?;
        if digest(&current) != plan.expected_sha256 {
            return Err(bad("LSP edit source changed; hash mismatch"));
        }
        apply_text(&current, &plan.edits)
    }
    pub fn apply(&self, plan: &EditPlan, ctx: &TurnContext) -> Result<Applied, AgentError> {
        let changed = self.preview(plan, ctx)?;
        let path = self.relative(&plan.uri)?;
        // Reject ambiguous symlink/hardlink replacement semantics before any write.
        let mut walk = PathBuf::new();
        for part in path.components() {
            walk.push(part);
            let m = self
                .dir
                .symlink_metadata(&walk)
                .map_err(|_| bad("cannot inspect edit path"))?;
            if m.file_type().is_symlink() {
                return Err(bad("LSP edits reject symlink paths"));
            }
        }
        let parent = self
            .dir
            .open_dir(
                path.parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
            )
            .map_err(|_| bad("cannot open edit parent"))?;
        let name = path.file_name().ok_or(bad("edit filename required"))?;
        let meta = parent
            .symlink_metadata(name)
            .map_err(|_| bad("cannot inspect edit target"))?;
        if !meta.is_file() {
            return Err(bad("regular edit target required"));
        }
        #[cfg(unix)]
        {
            use cap_std::fs::MetadataExt;
            if meta.nlink() != 1 {
                return Err(bad("LSP edits reject hardlinked targets"));
            }
        }
        let temp = format!(
            ".namzu-lsp-{}-{}",
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
        let mut file = parent
            .open_with(&temp, &options)
            .map_err(|_| bad("cannot create edit temporary file"))?;
        let result = (|| {
            file.set_permissions(meta.permissions())
                .map_err(|_| bad("cannot preserve edit permissions"))?;
            for chunk in changed.as_bytes().chunks(8192) {
                ctx.remaining()?;
                file.write_all(chunk)
                    .map_err(|_| bad("cannot write LSP edit"))?;
            }
            file.sync_all().map_err(|_| bad("cannot sync LSP edit"))?;
            let latest = self.read_in(&parent, Path::new(name), ctx)?;
            if digest(&latest) != plan.expected_sha256 {
                return Err(bad("edit source changed before replacement"));
            }
            let latest_meta = parent
                .symlink_metadata(name)
                .map_err(|_| bad("cannot recheck edit target"))?;
            if latest_meta.file_type().is_symlink() {
                return Err(bad("edit target became a symlink"));
            }
            ctx.remaining()?;
            parent
                .rename(&temp, &parent, name)
                .map_err(|_| bad("cannot replace LSP edit target"))?;
            Ok(Applied {
                uri: plan.uri.clone(),
                previous_sha256: plan.expected_sha256.clone(),
                sha256: digest(&changed),
                bytes: changed.len(),
            })
        })();
        drop(file);
        if result.is_err() {
            let _ = parent.remove_file(&temp);
        }
        result
    }
}
pub fn offset(text: &str, position: Position) -> Result<usize, AgentError> {
    let mut start = 0usize;
    for _ in 0..position.line {
        let relative = text[start..]
            .find(['\r', '\n'])
            .ok_or(bad("LSP line outside document"))?;
        let end = start + relative;
        start = end
            + if text.as_bytes()[end] == b'\r' && text.as_bytes().get(end + 1) == Some(&b'\n') {
                2
            } else {
                1
            };
    }
    let remaining = &text[start..];
    let end = remaining.find(['\r', '\n']).unwrap_or(remaining.len());
    let line = &remaining[..end];
    let mut units = 0;
    for (byte, c) in line.char_indices() {
        if units == position.character {
            return Ok(start + byte);
        }
        units += c.len_utf16() as u32;
        if units > position.character {
            return Err(bad("LSP position splits a UTF-16 surrogate"));
        }
    }
    if units == position.character {
        Ok(start + line.len())
    } else {
        Err(bad("LSP character outside line"))
    }
}
pub fn apply_text(text: &str, edits: &[TextEdit]) -> Result<String, AgentError> {
    if text.len() > DOCUMENT_BYTES
        || edits.is_empty()
        || edits.len() > 256
        || edits.iter().map(|e| e.new_text.len()).sum::<usize>() > DOCUMENT_BYTES
    {
        return Err(bad("LSP edit count or byte limit"));
    }
    let mut ranges = Vec::new();
    for edit in edits {
        let start = offset(text, edit.range.start)?;
        let end = offset(text, edit.range.end)?;
        if start > end {
            return Err(bad("reversed LSP edit range"));
        }
        ranges.push((start, end, &edit.new_text));
    }
    ranges.sort_by_key(|r| (r.0, r.1));
    for pair in ranges.windows(2) {
        if pair[1].0 < pair[0].1 || pair[1].0 == pair[0].0 {
            return Err(bad("overlapping or ambiguous LSP edits"));
        }
    }
    let new_len = ranges
        .iter()
        .try_fold(text.len(), |size, (start, end, replacement)| {
            size.checked_sub(end - start)
                .and_then(|s| s.checked_add(replacement.len()))
        })
        .ok_or(bad("LSP edit size overflow"))?;
    if new_len > DOCUMENT_BYTES {
        return Err(bad("edited document exceeds limit"));
    }
    let mut output = text.to_string();
    for (start, end, replacement) in ranges.into_iter().rev() {
        output.replace_range(start..end, replacement);
    }
    Ok(output)
}
