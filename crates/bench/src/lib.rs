//! Sequential, isolated-workspace benchmark with independent correctness oracles.
//! Workspaces are directories, not an OS sandbox. Only run trusted agent/test code.
use namzu_core::{AgentError, Cancellation, TurnContext};
use namzu_session::{PrivateStore, new_id, now_ms};
use namzu_tools::process::{ProcessSpec, run_process};
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
                || agent.args.iter().any(|s| s.len() > 65536)
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

fn oracle(task: &Task, workspace: &Path, cancellation: Cancellation) -> Result<bool, AgentError> {
    let root = cap_std::fs::Dir::open_ambient_dir(workspace, cap_std::ambient_authority())
        .map_err(|_| AgentError::State("cannot open oracle workspace"))?;
    let mut passed = true;
    for (name, expected) in &task.expected_files {
        let metadata = match root.symlink_metadata(name) {
            Ok(meta) => meta,
            Err(_) => {
                passed = false;
                continue;
            }
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 65536 {
            passed = false;
            continue;
        }
        let file = match root.open(name) {
            Ok(file) => file,
            Err(_) => {
                passed = false;
                continue;
            }
        };
        let mut bytes = vec![];
        file.take(65537)
            .read_to_end(&mut bytes)
            .map_err(|_| AgentError::State("cannot read oracle file"))?;
        if bytes != expected.as_bytes() {
            passed = false;
        }
    }
    if let Some(program) = task.check.first() {
        let args = task
            .check
            .iter()
            .skip(1)
            .map(|arg| arg.replace("{workspace}", &workspace.to_string_lossy()))
            .collect();
        let result = run_process(
            ProcessSpec {
                program: executable(Path::new(program))?,
                args,
                cwd: workspace.into(),
                env: env(&[]),
                timeout: Duration::from_secs(30),
                output_limit: 32768,
            },
            &TurnContext::new(Duration::from_secs(35), cancellation)?,
        )?;
        passed &= result.status == "exited" && result.exit_code == Some(0);
    }
    Ok(passed)
}

pub fn run(
    suite: &Suite,
    output_parent: &Path,
    cancellation: Cancellation,
    mut progress: impl FnMut(&Value),
) -> Result<Value, AgentError> {
    suite.validate()?;
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
    let mut report = json!({"version":1,"mode":suite.mode,"model":suite.model,"timeout_seconds":suite.timeout_seconds,"sequential":true,"started_ms":now_ms(),"suite_sha256":hash(&serde_json::to_vec(suite).map_err(|_|AgentError::Protocol("cannot hash suite"))?),"memory_scope":"wait4 ru_maxrss: largest individual process including waited children; not aggregate tree RSS/PSS","permission_profile_is_declared":true,"runs":[]});
    store.write("report.json", &report)?;
    for repeat in 0..suite.repeats {
        for task in &suite.tasks {
            // Alternate the serial ordering across repeats to reduce order bias.
            let order: Vec<_> = if repeat % 2 == 0 {
                (0..suite.agents.len()).collect()
            } else {
                (0..suite.agents.len()).rev().collect()
            };
            for agent_index in order {
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
                let result = run_process(
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
                )?;
                let elapsed_ms = started.elapsed().as_millis();
                let oracle_passed = oracle(task, &workspace, cancellation.clone())?;
                let status = if result.status != "exited" {
                    result.status.as_str()
                } else if result.exit_code != Some(0) {
                    "agent_error"
                } else if oracle_passed {
                    "passed"
                } else {
                    "incorrect"
                };
                let stats = serde_json::from_str::<Value>(result.stdout.trim())
                    .ok()
                    .and_then(|value|value.get("stats").cloned()).map(|value|json!({"provider_calls":value["provider_calls"].as_u64(),"tool_calls":value["tool_calls"].as_u64()}));
                let row = json!({"run_id":run_id,"execution_index":execution_index-1,"agent":agent.name,"task":task.id,"repeat":repeat,"status":status,"oracle_passed":oracle_passed,"exit_code":result.exit_code,"elapsed_ms":elapsed_ms,"peak_rss_kib":result.peak_rss_kib,"stats":stats,"prompt_sha256":hash(task.prompt.as_bytes()),"input_sha256":hash(&serde_json::to_vec(&task.files).map_err(|_|AgentError::Protocol("cannot hash inputs"))?),"program_sha256":program_hashes[agent_index],"permission_profile":agent.permission_profile});
                // Raw output stays in private ignored artifacts, never in the public report.
                store.write(
                    &format!("{run_id}.log.json"),
                    &json!({"stdout":result.stdout,"stderr":result.stderr}),
                )?;
                rows.push(row.clone());
                report["runs"] = json!(rows);
                store.write("report.json", &report)?;
                progress(&row);
            }
        }
    }
    report["finished_ms"] = json!(now_ms());
    report["output_directory"] = json!(directory);
    report["summary"]=json!(suite.agents.iter().map(|agent|{let runs=rows.iter().filter(|row|row["agent"]==agent.name).collect::<Vec<_>>();json!({"agent":agent.name,"passed":runs.iter().filter(|row|row["status"]=="passed").count(),"total":runs.len()})}).collect::<Vec<_>>());
    store.write("report.json", &report)?;
    Ok(report)
}

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
        _ => return Err(AgentError::Configuration("unknown built-in benchmark task")),
    };
    Ok(Task {
        id: id.into(),
        prompt: prompt.into(),
        files: BTreeMap::from([
            (
                "solution.py".into(),
                "# Implement the requested function here.\n".into(),
            ),
            ("README.md".into(), prompt.into()),
        ]),
        expected_files: BTreeMap::new(),
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
        _ => Err(AgentError::Configuration("unknown fixture task")),
    }
}
