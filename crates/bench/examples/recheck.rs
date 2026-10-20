//! Recheck recorded outputs after the refactor import-alias oracle fix, without model calls.
//! Usage: recheck ORIGINAL_SUITE ORIGINAL_RUN_DIRECTORY NEW_PRIVATE_OUTPUT_DIRECTORY
use namzu_bench::{Suite, builtin_task, oracle};
use namzu_core::Cancellation;
use namzu_session::PrivateStore;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

fn hash(value: &impl serde::Serialize) -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(value).unwrap()))
}
fn oracle_hash(task: &namzu_bench::Task) -> String {
    hash(&(
        &task.expected_files,
        &task.check,
        task.check_timeout_seconds,
        task.check_output_limit,
    ))
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if args.len() != 3 {
        return Err("expected suite, original run directory, new output directory".into());
    }
    let mut suite = Suite::load(&args[0])?;
    let root = args[1].canonicalize()?;
    let mut report: Value = serde_json::from_slice(&std::fs::read(root.join("report.json"))?)?;
    if report["version"] != 2
        || report["status"] != "complete"
        || report["suite_sha256"] != hash(&suite)
    {
        return Err("original report is incomplete or suite identity differs".into());
    }
    let rows = report["runs"].as_array().ok_or("missing runs")?;
    if rows.len() != suite.agents.len() * suite.tasks.len() * suite.repeats {
        return Err("incomplete run matrix".into());
    }
    // Check every path/identity before running any checker. No agent is launched.
    let mut selected = vec![];
    for (index, row) in rows.iter().enumerate() {
        if row["run_id"] != format!("run-{index}") || row["execution_index"] != index {
            return Err("invalid run identity".into());
        }
        let task = suite
            .tasks
            .iter()
            .position(|task| row["task"] == task.id)
            .ok_or("unknown task")?;
        if row["oracle_sha256"] != oracle_hash(&suite.tasks[task]) {
            return Err("original oracle identity differs".into());
        }
        let workspace = root.join(format!("run-{index}-workspace")).canonicalize()?;
        if workspace.parent() != Some(root.as_path()) {
            return Err("workspace escapes run directory".into());
        }
        selected.push((task, workspace));
    }
    let original_suite_hash = report["suite_sha256"].clone();
    // Only the checker source changes. Prompts, starting files and measured runs stay fixed.
    let fixed = builtin_task("refactor")?;
    let target = suite
        .tasks
        .iter_mut()
        .find(|task| task.id == "refactor")
        .ok_or("missing refactor task")?;
    target.check = fixed.check;
    suite.validate()?;
    if args[2].exists() {
        return Err("audit output directory must be new".into());
    }
    let store = PrivateStore::open(&args[2])?;
    store.write(
        "report.json",
        &json!({"version":2,"status":"running","kind":"oracle_recheck"}),
    )?;
    for (index, (task, workspace)) in selected.iter().enumerate() {
        let (result, logs) =
            oracle::evaluate(&suite.tasks[*task], workspace, Cancellation::default())?;
        let row = &mut report["runs"][index];
        row["original_status"] = row["status"].clone();
        row["original_oracle_passed"] = row["oracle_passed"].clone();
        row["original_oracle_sha256"] = row["oracle_sha256"].clone();
        let status = oracle::classify(
            row["execution_status"]
                .as_str()
                .ok_or("missing execution status")?,
            row["exit_code"]
                .as_i64()
                .and_then(|code| i32::try_from(code).ok()),
            &result,
        );
        row["status"] = json!(status);
        row["oracle_passed"] = json!(result.passed);
        row["oracle"] = json!(result);
        row["oracle_sha256"] = json!(oracle_hash(&suite.tasks[*task]));
        store.write(&format!("recheck-{index}.log.json"), &logs)?;
    }
    report["summary"] = json!(
        suite
            .agents
            .iter()
            .map(|agent| {
                let rows: Vec<_> = report["runs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|row| row["agent"] == agent.name)
                    .collect();
                let count =
                    |status: &str| rows.iter().filter(|row| row["status"] == status).count();
                json!({"agent":agent.name,"passed":count("passed"),"incorrect":count("incorrect"),
            "execution_errors":count("agent_error")+count("timeout")+count("output_limit"),
            "oracle_errors":count("oracle_error"),"total":rows.len()})
            })
            .collect::<Vec<_>>()
    );
    report["suite_sha256"] = json!(hash(&suite));
    report["audit"] = json!({"kind":"posthoc_oracle_recheck","all_runs_rechecked":selected.len(),
        "new_model_calls":0,"original_suite_sha256":original_suite_hash,
        "reason":"Accept direct function imports as valid delegation; preserve copied-logic rejection.",
        "agent_time_rss_and_stats_reused":true});
    store.write("report.json", &report)?;
    println!(
        "{}",
        json!({"rechecked":selected.len(),"summary":report["summary"]})
    );
    Ok(())
}
