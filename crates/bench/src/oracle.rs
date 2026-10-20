//! Deterministic gates. Missing measurements never become a zero correctness score.
use crate::{Task, env, executable};
use namzu_core::{AgentError, Cancellation, TurnContext};
use namzu_tools::process::{ProcessSpec, run_process};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::Path,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileOutcome {
    Passed,
    Missing,
    UnsafePath,
    TooLarge,
    Mismatch,
    ReadError,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FileResult {
    pub path: String,
    pub outcome: FileOutcome,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckOutcome {
    Passed,
    TestFailed,
    Timeout,
    OutputLimit,
    Error,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CheckResult {
    pub outcome: CheckOutcome,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OracleResult {
    /// None means at least one required measurement is unavailable.
    pub passed: Option<bool>,
    pub files: Vec<FileResult>,
    pub check: Option<CheckResult>,
    pub elapsed_ms: u128,
}

impl OracleResult {
    pub fn unavailable(&self) -> bool {
        self.passed.is_none()
    }
}

fn file_outcome(root: &cap_std::fs::Dir, name: &str, expected: &str) -> FileOutcome {
    let metadata = match root.symlink_metadata(name) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return FileOutcome::Missing,
        Err(_) => return FileOutcome::UnsafePath,
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return FileOutcome::UnsafePath;
    }
    if metadata.len() > 65536 {
        return FileOutcome::TooLarge;
    }
    let file = match root.open(name) {
        Ok(file) => file,
        Err(_) => return FileOutcome::ReadError,
    };
    let mut bytes = vec![];
    if file.take(65537).read_to_end(&mut bytes).is_err() {
        return FileOutcome::ReadError;
    }
    if bytes.len() > 65536 {
        FileOutcome::TooLarge
    } else if bytes == expected.as_bytes() {
        FileOutcome::Passed
    } else {
        FileOutcome::Mismatch
    }
}

/// All configured gates run; checker output is returned separately for private storage.
pub fn evaluate(
    task: &Task,
    workspace: &Path,
    cancellation: Cancellation,
) -> Result<(OracleResult, serde_json::Value), AgentError> {
    if (task.expected_files.is_empty() && task.check.is_empty())
        || !(1..=30).contains(&task.check_timeout_seconds)
        || !(1..=32768).contains(&task.check_output_limit)
        || task
            .expected_files
            .keys()
            .any(|path| !crate::safe_relative(path))
    {
        return Err(AgentError::Configuration("invalid oracle gates or limits"));
    }
    let context = TurnContext::new(
        Duration::from_secs(task.check_timeout_seconds + 5),
        cancellation,
    )?;
    context.remaining()?;
    let started = Instant::now();
    let root = cap_std::fs::Dir::open_ambient_dir(workspace, cap_std::ambient_authority())
        .map_err(|_| AgentError::State("cannot open oracle workspace"))?;
    let mut files = vec![];
    for (name, expected) in &task.expected_files {
        context.remaining()?;
        files.push(FileResult {
            path: name.clone(),
            outcome: file_outcome(&root, name, expected),
        });
    }
    let mut logs = serde_json::Value::Null;
    let check = if let Some(program) = task.check.first() {
        let result = executable(Path::new(program)).and_then(|program| {
            run_process(
                ProcessSpec {
                    program,
                    args: task
                        .check
                        .iter()
                        .skip(1)
                        .map(|arg| arg.replace("{workspace}", &workspace.to_string_lossy()))
                        .collect(),
                    cwd: workspace.into(),
                    env: env(&[]),
                    timeout: Duration::from_secs(task.check_timeout_seconds),
                    output_limit: task.check_output_limit,
                },
                &context,
            )
        });
        Some(match result {
            Ok(result) => {
                logs = serde_json::json!({"stdout":result.stdout,"stderr":result.stderr});
                let outcome = match (result.status.as_str(), result.exit_code) {
                    ("exited", Some(0)) => CheckOutcome::Passed,
                    ("exited", Some(_)) => CheckOutcome::TestFailed,
                    ("timeout", _) => CheckOutcome::Timeout,
                    ("output_limit", _) => CheckOutcome::OutputLimit,
                    _ => CheckOutcome::Error,
                };
                CheckResult {
                    outcome,
                    exit_code: result.exit_code,
                }
            }
            Err(AgentError::Cancelled) => return Err(AgentError::Cancelled),
            Err(_) => CheckResult {
                outcome: CheckOutcome::Error,
                exit_code: None,
            },
        })
    } else {
        None
    };
    let unavailable = files
        .iter()
        .any(|file| file.outcome == FileOutcome::ReadError)
        || check.as_ref().is_some_and(|check| {
            matches!(
                check.outcome,
                CheckOutcome::Timeout | CheckOutcome::OutputLimit | CheckOutcome::Error
            )
        });
    let passed = if unavailable {
        None
    } else {
        Some(
            files.iter().all(|file| file.outcome == FileOutcome::Passed)
                && check
                    .as_ref()
                    .is_none_or(|check| check.outcome == CheckOutcome::Passed),
        )
    };
    Ok((
        OracleResult {
            passed,
            files,
            check,
            elapsed_ms: started.elapsed().as_millis(),
        },
        logs,
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Passed,
    Incorrect,
    AgentError,
    Timeout,
    OutputLimit,
    OracleError,
}

pub fn classify(status: &str, exit_code: Option<i32>, oracle: &OracleResult) -> RunStatus {
    match status {
        "timeout" => RunStatus::Timeout,
        "output_limit" => RunStatus::OutputLimit,
        "exited" if exit_code == Some(0) => match oracle.passed {
            Some(true) => RunStatus::Passed,
            Some(false) => RunStatus::Incorrect,
            None => RunStatus::OracleError,
        },
        _ => RunStatus::AgentError,
    }
}
