//! Sequential, isolated-workspace benchmark with independent correctness oracles.
//! Workspaces are directories, not an OS sandbox. Only run trusted agent/test code.
pub mod oracle;
use namzu_core::{AgentError, Cancellation, TurnContext};
use namzu_session::{PrivateStore, new_id, now_ms};
use namzu_tools::process::{ProcessOutput, ProcessSpec, run_process};
pub use oracle::{CheckOutcome, FileOutcome, OracleResult, RunStatus};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Agent {
    pub name: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub permission_profile: String,
    #[serde(default)]
    pub inherit_env: Vec<String>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: String,
    pub prompt: String,
    pub files: BTreeMap<String, String>,
    #[serde(default)]
    pub expected_files: BTreeMap<String, String>,
    #[serde(default)]
    pub check: Vec<String>,
    #[serde(default = "check_timeout")]
    pub check_timeout_seconds: u64,
    #[serde(default = "check_output_limit")]
    pub check_output_limit: usize,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Suite {
    pub version: u32,
    pub mode: String,
    pub model: String,
    #[serde(default = "timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "once")]
    pub repeats: usize,
    pub agents: Vec<Agent>,
    pub tasks: Vec<Task>,
}
fn check_timeout() -> u64 {
    30
}
fn check_output_limit() -> usize {
    32768
}
fn timeout() -> u64 {
    300
}
fn once() -> usize {
    1
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
}
fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\0')
        && path.len() <= 4096
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn hash_program(path: &Path) -> Result<String, AgentError> {
    let mut file = std::fs::File::open(path)
        .map_err(|_| AgentError::State("cannot hash benchmark executable"))?;
    let mut digest = Sha256::new();
    let mut block = [0u8; 8192];
    loop {
        let size = file
            .read(&mut block)
            .map_err(|_| AgentError::State("cannot read benchmark executable"))?;
        if size == 0 {
            break;
        }
        digest.update(&block[..size]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub fn executable(path: &Path) -> Result<PathBuf, AgentError> {
    let selected = if path.components().count() == 1 && !path.is_absolute() {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|entry| entry.join(path))
            .find(|file| file.is_file())
            .ok_or(AgentError::Configuration("benchmark executable not found"))?
    } else {
        path.to_path_buf()
    };
    let selected = selected
        .canonicalize()
        .map_err(|_| AgentError::Configuration("cannot resolve benchmark executable"))?;
    if !selected.is_file() {
        return Err(AgentError::Configuration(
            "benchmark executable must be a file",
        ));
    }
    Ok(selected)
}

impl Suite {
    pub fn load(path: &Path) -> Result<Self, AgentError> {
        let file = std::fs::File::open(path)
            .map_err(|_| AgentError::Configuration("cannot open benchmark suite"))?;
        let mut bytes = vec![];
        file.take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| AgentError::Configuration("cannot read benchmark suite"))?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(AgentError::Configuration(
                "benchmark suite exceeds size limit",
            ));
        }
        let mut suite: Self = serde_json::from_slice(&bytes)
            .map_err(|_| AgentError::Configuration("invalid benchmark suite JSON"))?;
        let base = path
            .canonicalize()
            .map_err(|_| AgentError::Configuration("cannot resolve suite path"))?
            .parent()
            .unwrap()
            .to_path_buf();
        for agent in &mut suite.agents {
            if agent.program.components().count() > 1 && agent.program.is_relative() {
                agent.program = base.join(&agent.program);
            }
            agent.program = executable(&agent.program)?;
        }
        for task in &mut suite.tasks {
            if let Some(program) = task.check.first_mut() {
                *program = executable(Path::new(program))?
                    .to_string_lossy()
                    .into_owned();
            }
            for argument in task.check.iter_mut().skip(1) {
                *argument = argument.replace("{suite_dir}", &base.to_string_lossy());
            }
        }
        suite.validate()?;
        Ok(suite)
    }
    pub fn validate(&self) -> Result<(), AgentError> {
        if self.version != 1
            || !["fixture", "live"].contains(&self.mode.as_str())
            || self.model.is_empty()
            || self.model.len() > 256
            || !(1..=300).contains(&self.timeout_seconds)
            || !(1..=10).contains(&self.repeats)
            || self.agents.is_empty()
            || self.agents.len() > 8
            || self.tasks.is_empty()
            || self.tasks.len() > 16
        {
            return Err(AgentError::Configuration(
                "invalid benchmark version, mode or limits",
            ));
        }
        let mut names = BTreeSet::new();
        for agent in &self.agents {
            if !valid_id(&agent.name)
                || !names.insert(&agent.name)
                || agent.args.len() > 128
                || agent
                    .args
                    .iter()
                    .any(|s| s.len() > 65536 || s.contains('\0'))
                || agent.permission_profile != self.agents[0].permission_profile
                || agent.permission_profile.is_empty()
                || agent.inherit_env.len() > 64
                || agent.inherit_env.iter().any(|s| {
                    s.is_empty() || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                })
            {
                return Err(AgentError::Configuration(
                    "invalid agent, duplicate name or unequal declared permission profile",
                ));
            }
            if self.mode == "live" && !agent.args.iter().any(|arg| arg.contains("{model}")) {
                return Err(AgentError::Configuration(
                    "live agents must select the common {model} explicitly",
                ));
            }
        }
        let mut ids = BTreeSet::new();
        for task in &self.tasks {
            if !valid_id(&task.id)
                || !ids.insert(&task.id)
                || task.prompt.len() > 65536
                || task.files.len() > 128
                || task
                    .files
                    .iter()
                    .any(|(path, value)| !safe_relative(path) || value.len() > 65536)
                || task.files.values().map(String::len).sum::<usize>() > 1024 * 1024
                || (task.expected_files.is_empty() && task.check.is_empty())
                || task.expected_files.len() > 128
                || task
                    .expected_files
                    .iter()
                    .any(|(path, value)| !safe_relative(path) || value.len() > 65536)
                || task.check.len() > 128
                || task
                    .check
                    .iter()
                    .any(|arg| arg.len() > 65536 || arg.contains('\0'))
                || task.check.first().is_some_and(String::is_empty)
                || !(1..=30).contains(&task.check_timeout_seconds)
                || !(1..=32768).contains(&task.check_output_limit)
                || task.files.keys().any(|path| {
                    task.files
                        .keys()
                        .any(|other| other != path && Path::new(other).starts_with(path))
                })
            {
                return Err(AgentError::Configuration(
                    "invalid task paths, inputs or oracle",
                ));
            }
        }
        Ok(())
    }
}

fn expand(
    text: &str,
    workspace: &Path,
    state: &Path,
    task: &Task,
    suite: &Suite,
    run_id: &str,
) -> String {
    // Single substitution pass: placeholders embedded in a prompt are data.
    let values = [
        ("{workspace}", workspace.to_string_lossy().into_owned()),
        ("{state_dir}", state.to_string_lossy().into_owned()),
        ("{prompt}", task.prompt.clone()),
        ("{model}", suite.model.clone()),
        ("{task}", task.id.clone()),
        ("{run_id}", run_id.to_owned()),
    ];
    let mut out = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        if let Some((key, value)) = values.iter().find(|(key, _)| rest.starts_with(key)) {
            out.push_str(value);
            rest = &rest[key.len()..];
        } else {
            let c = rest.chars().next().unwrap();
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}
fn env(extra: &[String]) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    for name in [
        "PATH",
        "HOME",
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
    ]
    .into_iter()
    .chain(extra.iter().map(String::as_str))
    {
        if let Ok(value) = std::env::var(name) {
            values.insert(name.into(), value);
        }
    }
    values
}

pub fn run(
    suite: &Suite,
    output_parent: &Path,
    cancellation: Cancellation,
    mut progress: impl FnMut(&Value),
) -> Result<Value, AgentError> {
    suite.validate()?;
    if cancellation.is_cancelled() {
        return Err(AgentError::Cancelled);
    }
    // Resolve every checker before any agent effect, including direct SDK use.
    let mut tasks = suite.tasks.clone();
    for task in &mut tasks {
        if let Some(program) = task.check.first_mut() {
            *program = executable(Path::new(program))?
                .to_string_lossy()
                .into_owned();
        }
    }
    // Resolve commands before creating agent-controlled workspaces.
    let programs = suite
        .agents
        .iter()
        .map(|agent| executable(&agent.program))
        .collect::<Result<Vec<_>, _>>()?;
    let directory = output_parent.join(new_id());
    let store = PrivateStore::open(&directory)?;
    let directory = directory
        .canonicalize()
        .map_err(|_| AgentError::State("cannot resolve benchmark output"))?;
    let program_hashes = programs
        .iter()
        .map(|path| hash_program(path))
        .collect::<Result<Vec<_>, _>>()?;
    let mut rows = vec![];
    let mut execution_index = 0;
    let mut report = json!({"version":2,"status":"running","planned_runs":suite.repeats * suite.tasks.len() * suite.agents.len(),"mode":suite.mode,"model":suite.model,"timeout_seconds":suite.timeout_seconds,"sequential":true,"started_ms":now_ms(),"suite_sha256":hash(&serde_json::to_vec(suite).map_err(|_|AgentError::Protocol("cannot hash suite"))?),"memory_scope":"wait4 ru_maxrss: largest individual process including waited children; not aggregate tree RSS/PSS","permission_profile_is_declared":true,"runs":[]});
    store.write("report.json", &report)?;
    for repeat in 0..suite.repeats {
        for task in &tasks {
            if cancellation.is_cancelled() {
                return Err(AgentError::Cancelled);
            }
            // Alternate the serial ordering across repeats to reduce order bias.
            let order: Vec<_> = if repeat % 2 == 0 {
                (0..suite.agents.len()).collect()
            } else {
                (0..suite.agents.len()).rev().collect()
            };
            for agent_index in order {
                if cancellation.is_cancelled() {
                    return Err(AgentError::Cancelled);
                }
                let agent = &suite.agents[agent_index];
                let run_id = format!("run-{execution_index}");
                execution_index += 1;
                let workspace = directory.join(format!("{run_id}-workspace"));
                let state = directory.join(format!("{run_id}-state"));
                std::fs::create_dir(&workspace)
                    .map_err(|_| AgentError::State("cannot create fresh benchmark workspace"))?;
                for (name, content) in &task.files {
                    let path = workspace.join(name);
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent)
                            .map_err(|_| AgentError::State("cannot create task input directory"))?;
                    }
                    std::fs::write(path, content)
                        .map_err(|_| AgentError::State("cannot write task input"))?;
                }
                let args = agent
                    .args
                    .iter()
                    .map(|arg| expand(arg, &workspace, &state, task, suite, &run_id))
                    .collect();
                let started = Instant::now();
                let result = match run_process(
                    ProcessSpec {
                        program: programs[agent_index].clone(),
                        args,
                        cwd: workspace.clone(),
                        env: env(&agent.inherit_env),
                        timeout: Duration::from_secs(suite.timeout_seconds),
                        output_limit: 1024 * 1024,
                    },
                    &TurnContext::new(
                        Duration::from_secs(suite.timeout_seconds + 5),
                        cancellation.clone(),
                    )?,
                ) {
                    Ok(result) => result,
                    Err(AgentError::Cancelled) => return Err(AgentError::Cancelled),
                    Err(_) => ProcessOutput {
                        status: "error".into(),
                        exit_code: None,
                        stdout: String::new(),
                        stderr: "process supervision failed".into(),
                        peak_rss_kib: None,
                    },
                };
                let elapsed_ms = started.elapsed().as_millis();
                let (oracle, oracle_logs) =
                    oracle::evaluate(task, &workspace, cancellation.clone())?;
                let status = oracle::classify(&result.status, result.exit_code, &oracle);
                let stats = serde_json::from_str::<Value>(result.stdout.trim())
                    .ok()
                    .and_then(|value|value.get("stats").cloned()).map(|value|json!({"provider_calls":value["provider_calls"].as_u64(),"tool_calls":value["tool_calls"].as_u64()}));
                let row = json!({"run_id":run_id,"execution_index":execution_index-1,"agent":agent.name,"task":task.id,"repeat":repeat,"status":status,"execution_status":result.status,"oracle_passed":oracle.passed,"oracle":oracle,"oracle_sha256":hash(&serde_json::to_vec(&(task.expected_files.clone(),task.check.clone(),task.check_timeout_seconds,task.check_output_limit)).map_err(|_|AgentError::Protocol("cannot hash oracle"))?),"exit_code":result.exit_code,"elapsed_ms":elapsed_ms,"peak_rss_kib":result.peak_rss_kib,"stats":stats,"prompt_sha256":hash(task.prompt.as_bytes()),"input_sha256":hash(&serde_json::to_vec(&task.files).map_err(|_|AgentError::Protocol("cannot hash inputs"))?),"program_sha256":program_hashes[agent_index],"permission_profile":agent.permission_profile});
                // Raw output stays in private ignored artifacts, never in the public report.
                store.write(
                    &format!("{run_id}.log.json"),
                    &json!({"stdout":result.stdout,"stderr":result.stderr,"oracle":oracle_logs}),
                )?;
                rows.push(row.clone());
                report["runs"] = json!(rows);
                store.write("report.json", &report)?;
                progress(&row);
            }
        }
    }
    report["status"] = json!("complete");
    report["finished_ms"] = json!(now_ms());
    report["output_directory"] = json!(directory);
    report["summary"]=json!(suite.agents.iter().map(|agent|{let runs=rows.iter().filter(|row|row["agent"]==agent.name).collect::<Vec<_>>();json!({"agent":agent.name,"passed":runs.iter().filter(|row|row["status"]=="passed").count(),"total":runs.len(),"incorrect":runs.iter().filter(|row|row["status"]=="incorrect").count(),"oracle_errors":runs.iter().filter(|row|row["status"]=="oracle_error").count(),"execution_errors":runs.iter().filter(|row|["agent_error","timeout","output_limit"].iter().any(|status|row["status"]==*status)).count()})}).collect::<Vec<_>>());
    store.write("report.json", &report)?;
    Ok(report)
}

/// The suite hash pins prompts, inputs and embedded checker sources; no inference.
pub const BUILTIN_TASKS: &[&str] = &["slug", "intervals", "toposort", "ledger", "refactor"];

pub fn builtin_task(id: &str) -> Result<Task, AgentError> {
    let (prompt, check) = match id {
        "slug" => (
            "Implement solution.py slugify(text): NFKD normalize, discard non-ASCII characters, lowercase, replace runs of characters other than a-z and 0-9 with one hyphen, strip edge hyphens. Empty input returns ''. Do not modify the input. Use only Python standard library.",
            include_str!("../../../bench/oracles/slug.py"),
        ),
        "intervals" => (
            "Implement solution.py merge_intervals(intervals). Inputs are pairs of integer endpoints [start,end). Endpoints must have exact Python type int: reject bool, float and every other endpoint type with TypeError. Reject reversed endpoints with ValueError. Drop zero-length intervals; sort without mutating input; merge overlapping or touching intervals. Return sorted list of (start,end) tuples. Use only Python standard library.",
            include_str!("../../../bench/oracles/intervals.py"),
        ),
        "toposort" => (
            "Implement solution.py plan(nodes, edges). Node names are strings, duplicate nodes/edges are ignored. Edges (a,b) mean a must precede b. Always choose the lexicographically smallest currently available node. Reject unknown edge nodes and cycles with ValueError. Do not mutate inputs. Return list of nodes. Use only Python standard library.",
            include_str!("../../../bench/oracles/toposort.py"),
        ),
        "ledger" => (
            "Fix solution.py ledger(rows). Each row is (id, account, amount). Validate EVERY row, including duplicates: exact str id/account and exact int amount (bool/float forbidden), otherwise TypeError. For repeated ids keep only the FIRST row, even across accounts. Sum signed amounts by account; retain zero totals. Return a dict with account keys inserted in sorted order. Do not mutate rows. Standard library only.",
            include_str!("../../../bench/oracles/ledger.py"),
        ),
        "refactor" => (
            "Refactor solution.py normalize_many(texts) to remove duplicated normalization logic by delegating to legacy.normalize once per input element, in order, including duplicates. Keep legacy.py byte-for-byte unchanged. Preserve list output, Unicode behavior, TypeError, input immutability and generator support. Standard library only.",
            include_str!("../../../bench/oracles/refactor.py"),
        ),
        _ => return Err(AgentError::Configuration("unknown built-in benchmark task")),
    };
    let mut files = BTreeMap::from([
        (
            "solution.py".into(),
            "# Implement the requested function here.\n".into(),
        ),
        ("README.md".into(), prompt.into()),
    ]);
    let mut expected_files = BTreeMap::new();
    if id == "ledger" {
        files.insert(
            "solution.py".into(),
            "def ledger(rows):\n    return {account: amount for _, account, amount in rows}\n"
                .into(),
        );
    }
    if id == "refactor" {
        let legacy = include_str!("../../../bench/fixtures/legacy.py").to_owned();
        files.insert("legacy.py".into(), legacy.clone());
        expected_files.insert("legacy.py".into(), legacy);
        files.insert("solution.py".into(), "def normalize_many(texts):\n    return [\" \".join(text.strip().lower().split()) for text in texts]\n".into());
    }
    Ok(Task {
        id: id.into(),
        prompt: prompt.into(),
        files,
        expected_files,
        check_timeout_seconds: check_timeout(),
        check_output_limit: check_output_limit(),
        check: vec![
            executable(Path::new("python3"))?
                .to_string_lossy()
                .into_owned(),
            "-I".into(),
            "-c".into(),
            check.into(),
        ],
    })
}
pub fn fixture_solution(id: &str) -> Result<&'static str, AgentError> {
    match id {
        "slug" => Ok(include_str!("../../../bench/fixtures/slug.py")),
        "intervals" => Ok(include_str!("../../../bench/fixtures/intervals.py")),
        "toposort" => Ok(include_str!("../../../bench/fixtures/toposort.py")),
        "ledger" => Ok(include_str!("../../../bench/fixtures/ledger.py")),
        "refactor" => Ok(include_str!("../../../bench/fixtures/refactor.py")),
        _ => Err(AgentError::Configuration("unknown fixture task")),
    }
}
