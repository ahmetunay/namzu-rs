use crate::{
    FileTools, MAX_FILE,
    process::{ProcessSpec, run_process},
    valid_path,
};
use namzu_core::{AgentError, ToolCall, ToolDefinition, ToolResult, Tools, TurnContext};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Debug, Default, Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ToolPolicy {
    pub allow_write: bool,
    pub allow_shell: bool,
    pub allow: Option<BTreeSet<String>>,
    pub deny: BTreeSet<String>,
}
impl ToolPolicy {
    fn admits(&self, name: &str) -> bool {
        !self.deny.contains(name) && self.allow.as_ref().is_none_or(|set| set.contains(name))
    }
}

pub struct TerminalTools {
    files: FileTools,
    root: PathBuf,
    policy: ToolPolicy,
}
impl TerminalTools {
    pub fn new(root: impl AsRef<Path>, policy: ToolPolicy) -> std::io::Result<Self> {
        let root = root.as_ref().canonicalize()?;
        Ok(Self {
            files: FileTools::new(&root, policy.allow_write)?,
            root,
            policy,
        })
    }
    fn definition(
        name: &str,
        description: &str,
        properties: serde_json::Value,
        required: &[&str],
    ) -> ToolDefinition {
        ToolDefinition {
            name: name.into(),
            description: description.into(),
            parameters: json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
        }
    }
    fn read(&mut self, path: &str, context: &TurnContext) -> Result<ToolResult, AgentError> {
        self.files.call(
            &ToolCall {
                id: "internal".into(),
                name: "read_file".into(),
                arguments: json!({"path":path}),
            },
            context,
        )
    }
    fn execute(
        &mut self,
        call: &ToolCall,
        context: &TurnContext,
    ) -> Result<ToolResult, AgentError> {
        macro_rules! args {
            ($ty:ty) => {
                match serde_json::from_value::<$ty>(call.arguments.clone()) {
                    Ok(value) => value,
                    Err(_) => return Ok(ToolResult::error("invalid tool arguments")),
                }
            };
        }
        match call.name.as_str() {
            "shell" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Args {
                    command: String,
                    #[serde(default = "dot")]
                    cwd: String,
                    #[serde(default = "shell_timeout")]
                    timeout_seconds: u64,
                }
                let args = args!(Args);
                if args.command.is_empty()
                    || args.command.len() > MAX_FILE
                    || !(1..=300).contains(&args.timeout_seconds)
                    || valid_path(&args.cwd).is_err()
                {
                    return Ok(ToolResult::error("invalid shell command, cwd or timeout"));
                }
                let cwd = match self.root.join(&args.cwd).canonicalize() {
                    Ok(path) if path.starts_with(&self.root) && path.is_dir() => path,
                    _ => return Ok(ToolResult::error("shell cwd must be within workspace")),
                };
                let mut env = BTreeMap::new();
                for name in [
                    "PATH",
                    "LANG",
                    "LC_ALL",
                    "HTTP_PROXY",
                    "HTTPS_PROXY",
                    "ALL_PROXY",
                    "NO_PROXY",
                    "http_proxy",
                    "https_proxy",
                    "no_proxy",
                    "SSL_CERT_FILE",
                    "SSL_CERT_DIR",
                    "REQUESTS_CA_BUNDLE",
                ] {
                    if let Ok(value) = std::env::var(name) {
                        env.insert(name.into(), value);
                    }
                }
                let output = run_process(
                    ProcessSpec {
                        program: "/bin/sh".into(),
                        args: vec!["-c".into(), args.command],
                        cwd,
                        env,
                        timeout: Duration::from_secs(args.timeout_seconds),
                        output_limit: MAX_FILE,
                    },
                    context,
                )?;
                let error = output.status != "exited" || output.exit_code != Some(0);
                Ok(ToolResult {
                    content: serde_json::to_string(&output)
                        .map_err(|_| AgentError::Protocol("cannot encode process output"))?,
                    is_error: error,
                })
            }
            "search_files" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Args {
                    #[serde(default = "dot")]
                    path: String,
                    query: String,
                }
                let args = args!(Args);
                if args.query.is_empty()
                    || args.query.len() > 4096
                    || valid_path(&args.path).is_err()
                {
                    return Ok(ToolResult::error(
                        "expected bounded nonempty literal query and relative path",
                    ));
                }
                let mut budget = SearchBudget {
                    entries: 0,
                    bytes: 0,
                    matches: vec![],
                    truncated: false,
                };
                let result = search(
                    &self.files.root,
                    &args.path,
                    &args.query,
                    &mut budget,
                    context,
                    0,
                );
                match result {
                    Ok(()) => Ok(ToolResult::ok(
                        json!({"matches":budget.matches,"truncated":budget.truncated}).to_string(),
                    )),
                    Err(AgentError::Cancelled) => Err(AgentError::Cancelled),
                    Err(AgentError::Timeout) => Err(AgentError::Timeout),
                    Err(_) => Ok(ToolResult::error("cannot search workspace path")),
                }
            }
            "apply_patch" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Args {
                    path: String,
                    old_text: String,
                    new_text: String,
                }
                let args = args!(Args);
                if args.old_text.is_empty()
                    || args.old_text.len() > MAX_FILE
                    || args.new_text.len() > MAX_FILE
                {
                    return Ok(ToolResult::error(
                        "expected nonempty old_text and bounded replacement",
                    ));
                }
                let current = self.read(&args.path, context)?;
                if current.is_error {
                    return Ok(current);
                }
                if current.content.match_indices(&args.old_text).count() != 1 {
                    return Ok(ToolResult::error(
                        "patch must match exactly once; file unchanged",
                    ));
                }
                let changed = current.content.replacen(&args.old_text, &args.new_text, 1);
                if changed.len() > MAX_FILE {
                    return Ok(ToolResult::error("patched file exceeds 64 KiB"));
                }
                let diff = diff(&args.path, &current.content, &changed);
                // Guard against changes observed between validation and replacement.
                let check = self.read(&args.path, context)?;
                if check.is_error || check.content != current.content {
                    return Ok(ToolResult::error(
                        "file changed during patch; no replacement made",
                    ));
                }
                let result = self.files.call(
                    &ToolCall {
                        id: call.id.clone(),
                        name: "write_file".into(),
                        arguments: json!({"path":args.path,"content":changed}),
                    },
                    context,
                )?;
                if result.is_error {
                    Ok(result)
                } else {
                    Ok(ToolResult::ok(diff))
                }
            }
            "diff_file" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Args {
                    path: String,
                    content: String,
                }
                let args = args!(Args);
                if args.content.len() > MAX_FILE {
                    return Ok(ToolResult::error("proposed content exceeds 64 KiB"));
                }
                let current = self.read(&args.path, context)?;
                if current.is_error {
                    Ok(current)
                } else {
                    Ok(ToolResult::ok(diff(
                        &args.path,
                        &current.content,
                        &args.content,
                    )))
                }
            }
            _ => self.files.call(call, context),
        }
    }
}
fn dot() -> String {
    ".".into()
}
fn shell_timeout() -> u64 {
    60
}
fn diff(path: &str, old: &str, new: &str) -> String {
    let value = similar::TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string();
    if value.len() > MAX_FILE {
        format!(
            "diff exceeds 64 KiB; old_bytes={}, new_bytes={}",
            old.len(),
            new.len()
        )
    } else {
        value
    }
}
struct SearchBudget {
    entries: usize,
    bytes: usize,
    matches: Vec<serde_json::Value>,
    truncated: bool,
}
fn search(
    root: &cap_std::fs::Dir,
    path: &str,
    query: &str,
    budget: &mut SearchBudget,
    context: &TurnContext,
    depth: usize,
) -> Result<(), AgentError> {
    context.remaining()?;
    if depth > 32
        || budget.entries >= 2048
        || budget.bytes >= 4 * 1024 * 1024
        || budget.matches.len() >= 64
    {
        budget.truncated = true;
        return Ok(());
    }
    let error = || AgentError::Protocol("cannot traverse search path");
    let meta = root.symlink_metadata(path).map_err(|_| error())?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    if meta.is_dir() {
        let mut entries = vec![];
        for entry in root.read_dir(path).map_err(|_| error())? {
            context.remaining()?;
            if entries.len() >= 2048 {
                budget.truncated = true;
                break;
            }
            entries.push(entry.map_err(|_| error())?.file_name());
        }
        entries.sort();
        for name in entries {
            if [".git", "target", "node_modules", ".namzu"]
                .iter()
                .any(|skip| name == *skip)
            {
                continue;
            }
            budget.entries += 1;
            if budget.entries >= 2048
                || budget.bytes >= 4 * 1024 * 1024
                || budget.matches.len() >= 64
            {
                budget.truncated = true;
                break;
            }
            let child = Path::new(path).join(name);
            if let Some(child) = child.to_str() {
                search(root, child, query, budget, context, depth + 1)?;
            }
        }
    } else if meta.is_file() && meta.len() <= MAX_FILE as u64 {
        use std::io::Read;
        let mut options = cap_std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = root.open_with(path, &options).map_err(|_| error())?;
        if !file.metadata().map_err(|_| error())?.is_file() {
            return Ok(());
        }
        let mut bytes = vec![];
        file.take((MAX_FILE + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| error())?;
        budget.bytes += bytes.len();
        if bytes.len() > MAX_FILE {
            return Ok(());
        }
        if let Ok(text) = std::str::from_utf8(&bytes) {
            for (index, line) in text.lines().enumerate() {
                if line.contains(query) {
                    budget.matches.push(json!({"path":path,"line":index+1,"text":line.chars().take(512).collect::<String>()}));
                    if budget.matches.len() >= 64 {
                        budget.truncated = true;
                        break;
                    }
                }
            }
        }
    }
    Ok(())
}
impl Tools for TerminalTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        let mut list = self.files.definitions();
        list.push(Self::definition("search_files","Search a literal string in workspace UTF-8 files; bounded traversal, skips symlinks and generated directories.",json!({"path":{"type":"string"},"query":{"type":"string"}}),&["query"]));
        list.push(Self::definition(
            "diff_file",
            "Preview a unified diff without changing the file.",
            json!({"path":{"type":"string"},"content":{"type":"string"}}),
            &["path", "content"],
        ));
        if self.policy.allow_write {
            list.push(Self::definition("apply_patch","Replace old_text only if it matches exactly once; returns a diff. Bounded single-file edit.",json!({"path":{"type":"string"},"old_text":{"type":"string"},"new_text":{"type":"string"}}),&["path","old_text","new_text"]));
        }
        if self.policy.allow_shell {
            list.push(Self::definition("shell","Execute host /bin/sh in a workspace cwd. This grant permits host filesystem/network access; it is not a sandbox. Output and process lifetime are bounded.",json!({"command":{"type":"string"},"cwd":{"type":"string"},"timeout_seconds":{"type":"integer","minimum":1,"maximum":300}}),&["command"]));
        }
        list.into_iter()
            .filter(|d| self.policy.admits(&d.name))
            .collect()
    }
    fn call(&mut self, call: &ToolCall, context: &TurnContext) -> Result<ToolResult, AgentError> {
        context.remaining()?;
        if !self.definitions().iter().any(|d| d.name == call.name) {
            return Ok(ToolResult::error("tool denied or unavailable"));
        }
        self.execute(call, context)
    }
}
