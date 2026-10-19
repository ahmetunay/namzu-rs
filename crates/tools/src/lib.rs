//! Filesystem capabilities and explicitly granted host terminal tools.
pub mod content;
pub mod process;
mod terminal;
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use namzu_core::{AgentError, ToolCall, ToolDefinition, ToolResult, Tools, TurnContext};
use serde::Deserialize;
use serde_json::json;
use std::{
    io::{Read, Write},
    path::{Component, Path},
    sync::atomic::{AtomicU64, Ordering},
};
pub use terminal::{TerminalTools, ToolPolicy};

const MAX_FILE: usize = 64 * 1024;
const MAX_ENTRIES: usize = 512;
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

pub struct FileTools {
    root: Dir,
    content: content::ContentReader,
    writable: bool,
}
impl FileTools {
    pub fn new(root: impl AsRef<Path>, writable: bool) -> std::io::Result<Self> {
        let root = Dir::open_ambient_dir(root, ambient_authority())?;
        Ok(Self {
            content: content::ContentReader::from_dir(root.try_clone()?)?,
            root,
            writable,
        })
    }

    fn execute(&self, call: &ToolCall, context: &TurnContext) -> Result<String, &'static str> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct FileArgs {
            path: String,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WriteArgs {
            path: String,
            content: String,
        }
        match call.name.as_str() {
            "read_file" => {
                let args: FileArgs =
                    serde_json::from_value(call.arguments.clone()).map_err(|_| "expected path")?;
                valid_path(&args.path)?;
                let mut options = OpenOptions::new();
                options.read(true);
                #[cfg(unix)]
                {
                    use cap_std::fs::OpenOptionsExt;
                    options.custom_flags(libc::O_NONBLOCK);
                }
                let file = self
                    .root
                    .open_with(&args.path, &options)
                    .map_err(|_| "cannot open file within workspace")?;
                if !file
                    .metadata()
                    .map_err(|_| "cannot inspect file")?
                    .is_file()
                {
                    return Err("expected regular file");
                }
                let mut content = String::new();
                file.take((MAX_FILE + 1) as u64)
                    .read_to_string(&mut content)
                    .map_err(|_| "cannot read UTF-8 file")?;
                if content.len() > MAX_FILE {
                    return Err("file exceeds 64 KiB");
                }
                Ok(content)
            }
            "list_files" => {
                let args: FileArgs =
                    serde_json::from_value(call.arguments.clone()).map_err(|_| "expected path")?;
                valid_path(&args.path)?;
                let mut names = Vec::new();
                let entries = self
                    .root
                    .read_dir(&args.path)
                    .map_err(|_| "cannot list directory within workspace")?;
                let mut total = 0;
                for entry in entries {
                    let name = entry
                        .map_err(|_| "cannot inspect directory entry")?
                        .file_name()
                        .to_string_lossy()
                        .into_owned();
                    total += name.len();
                    if names.len() == MAX_ENTRIES || total > MAX_FILE {
                        return Err("directory listing exceeds limit");
                    }
                    names.push(name);
                }
                names.sort();
                serde_json::to_string(&names).map_err(|_| "cannot encode listing")
            }
            "write_file" if self.writable => {
                let args: WriteArgs = serde_json::from_value(call.arguments.clone())
                    .map_err(|_| "expected path and content")?;
                valid_path(&args.path)?;
                if args.content.len() > MAX_FILE {
                    return Err("content exceeds 64 KiB");
                }
                let path = Path::new(&args.path);
                let parent = self
                    .root
                    .open_dir(
                        path.parent()
                            .filter(|p| !p.as_os_str().is_empty())
                            .unwrap_or(Path::new(".")),
                    )
                    .map_err(|_| "cannot open parent directory within workspace")?;
                let basename = path.file_name().ok_or("expected file name")?;
                let temporary = format!(
                    ".namzu-write-{}-{}",
                    std::process::id(),
                    TEMP_ID.fetch_add(1, Ordering::Relaxed)
                );
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                let mut file = parent
                    .open_with(&temporary, &options)
                    .map_err(|_| "cannot create temporary file")?;
                let result = (|| {
                    if let Ok(metadata) = parent.symlink_metadata(basename)
                        && metadata.is_file()
                    {
                        file.set_permissions(metadata.permissions())
                            .map_err(|_| "cannot preserve file permissions")?;
                    }
                    file.write_all(args.content.as_bytes())
                        .map_err(|_| "cannot write temporary file")?;
                    file.sync_all().map_err(|_| "cannot sync temporary file")?;
                    context
                        .remaining()
                        .map_err(|_| "turn stopped before file replacement")?;
                    parent
                        .rename(&temporary, &parent, basename)
                        .map_err(|_| "cannot replace file")?;
                    Ok(format!("wrote {} bytes", args.content.len()))
                })();
                if result.is_err() {
                    let _ = parent.remove_file(&temporary);
                }
                result
            }
            _ => Err("tool is unavailable"),
        }
    }
}

fn valid_path(path: &str) -> Result<(), &'static str> {
    if path.is_empty()
        || path.len() > 4096
        || Path::new(path).components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("expected relative workspace path without parent traversal");
    }
    Ok(())
}

impl Tools for FileTools {
    fn validate(&mut self, call: &ToolCall, context: &TurnContext) -> Result<(), AgentError> {
        context.remaining()?;
        if content::CONTENT_NAMES.contains(&call.name.as_str()) {
            content::validate(call)?;
        }
        Ok(())
    }
    fn definitions(&self) -> Vec<ToolDefinition> {
        let mut definitions = vec![];
        for (name, description) in [
            (
                "read_file",
                "Read a UTF-8 file, at most 64 KiB, within the workspace.",
            ),
            (
                "list_files",
                "List at most 512 names in a workspace directory. Use '.' for the root.",
            ),
        ] {
            definitions.push(ToolDefinition { name: name.into(), description: description.into(), parameters: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}) });
        }
        definitions.extend(content::definitions());
        if self.writable {
            definitions.push(ToolDefinition { name: "write_file".into(), description: "Atomically replace a workspace file with UTF-8 text up to 64 KiB. Parent directory must exist.".into(), parameters: json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false}) });
        }
        definitions
    }
    fn call(&mut self, call: &ToolCall, context: &TurnContext) -> Result<ToolResult, AgentError> {
        context.remaining()?;
        if content::CONTENT_NAMES.contains(&call.name.as_str()) {
            let result = self.content.call(call, context);
            context.remaining()?;
            return match result {
                Err(AgentError::Configuration(message)) => Ok(ToolResult::error(message)),
                other => other,
            };
        }
        let result = match self.execute(call, context) {
            Ok(text) => ToolResult::ok(text),
            Err(error) => ToolResult::error(error),
        };
        context.remaining()?;
        Ok(result)
    }
}
