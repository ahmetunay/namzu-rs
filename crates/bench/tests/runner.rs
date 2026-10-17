use namzu_bench::*;
use namzu_core::Cancellation;
use std::{collections::BTreeMap, path::PathBuf};

fn agent(name: &str, command: &str) -> Agent {
    Agent {
        name: name.into(),
        program: "/bin/sh".into(),
        args: vec!["-c".into(), command.into()],
        permission_profile: "fixture-shell".into(),
        inherit_env: vec![],
    }
}
fn task() -> Task {
    Task {
        id: "file".into(),
        prompt: "write exactly 12\\n; {model} is literal prompt data".into(),
        files: BTreeMap::new(),
        expected_files: BTreeMap::from([("answer.txt".into(), "12\n".into())]),
        check: vec![],
    }
}
fn suite() -> Suite {
    Suite {
        version: 1,
        mode: "fixture".into(),
        model: "fixture".into(),
        timeout_seconds: 300,
        repeats: 1,
        agents: vec![
            agent("correct", "printf '12\\n' > answer.txt"),
            agent("liar", "printf done"),
        ],
        tasks: vec![task()],
    }
}

#[test]
fn correctness_comes_from_files_not_done_message_with_fresh_workspaces() {
    let root = tempfile::tempdir().unwrap();
    let report = run(&suite(), root.path(), Cancellation::default(), |_| {}).unwrap();
    assert_eq!(report["summary"][0]["passed"], 1);
    assert_eq!(report["summary"][1]["passed"], 0);
    assert_eq!(report["runs"][0]["status"], "passed");
    assert_eq!(report["runs"][1]["status"], "incorrect");
    assert_eq!(
        report["runs"][0]["prompt_sha256"],
        report["runs"][1]["prompt_sha256"]
    );
    assert_eq!(
        report["runs"][0]["input_sha256"],
        report["runs"][1]["input_sha256"]
    );
    assert!(report["runs"][0]["peak_rss_kib"].as_u64().unwrap() > 0);
    assert!(
        PathBuf::from(report["output_directory"].as_str().unwrap())
            .join("report.json")
            .is_file()
    );
}

#[test]
fn repeats_alternate_agent_order_but_remain_serial() {
    let root = tempfile::tempdir().unwrap();
    let mut spec = suite();
    spec.repeats = 2;
    let report = run(&spec, root.path(), Cancellation::default(), |_| {}).unwrap();
    let order = report["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["agent"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(order, vec!["correct", "liar", "liar", "correct"]);
    assert_eq!(report["sequential"], true);
}

#[test]
fn unequal_permissions_bad_paths_duplicate_names_and_missing_model_selection_fail_preflight() {
    let mut spec = suite();
    spec.agents[1].permission_profile = "other".into();
    assert!(spec.validate().is_err());
    let mut spec = suite();
    spec.tasks[0].files.insert("../escape".into(), "bad".into());
    assert!(spec.validate().is_err());
    let mut spec = suite();
    spec.agents[1].name = "correct".into();
    assert!(spec.validate().is_err());
    let mut spec = suite();
    spec.mode = "live".into();
    assert!(spec.validate().is_err());
    let mut spec = suite();
    spec.timeout_seconds = 301;
    assert!(spec.validate().is_err());
}

#[test]
fn nonzero_exit_is_agent_error_even_if_oracle_passes() {
    let root = tempfile::tempdir().unwrap();
    let mut spec = suite();
    spec.agents = vec![agent("fail", "printf '12\\n' > answer.txt; exit 9")];
    let report = run(&spec, root.path(), Cancellation::default(), |_| {}).unwrap();
    assert_eq!(report["runs"][0]["oracle_passed"], true);
    assert_eq!(report["runs"][0]["status"], "agent_error");
}

#[test]
fn independent_checker_failure_cannot_be_overridden_by_output() {
    let root = tempfile::tempdir().unwrap();
    let mut spec = suite();
    spec.tasks[0].check = vec!["/bin/sh".into(), "-c".into(), "exit 1".into()];
    let report = run(&spec, root.path(), Cancellation::default(), |_| {}).unwrap();
    assert_eq!(report["summary"][0]["passed"], 0);
}

#[test]
fn cancellation_before_start_leaves_no_agent_effect() {
    let root = tempfile::tempdir().unwrap();
    let cancellation = Cancellation::default();
    cancellation.cancel();
    assert_eq!(
        run(&suite(), root.path(), cancellation, |_| {}).unwrap_err(),
        namzu_core::AgentError::Cancelled
    );
}

#[test]
fn prompt_placeholders_are_not_recursively_expanded_or_shell_interpreted() {
    let root = tempfile::tempdir().unwrap();
    let mut spec = suite();
    spec.agents = vec![Agent {
        name: "argument".into(),
        program: "/bin/sh".into(),
        args: vec![
            "-c".into(),
            "printf '%s' \"$1\" > answer.txt".into(),
            "arg0".into(),
            "{prompt}".into(),
        ],
        permission_profile: "fixture-shell".into(),
        inherit_env: vec![],
    }];
    spec.tasks[0].prompt = "{model} $(touch escaped) `touch escaped2`".into();
    let expected = spec.tasks[0].prompt.clone();
    spec.tasks[0]
        .expected_files
        .insert("answer.txt".into(), expected);
    let report = run(&spec, root.path(), Cancellation::default(), |_| {}).unwrap();
    assert_eq!(report["summary"][0]["passed"], 1);
    let workspace =
        PathBuf::from(report["output_directory"].as_str().unwrap()).join("run-0-workspace");
    assert!(!workspace.join("escaped").exists());
    assert!(!workspace.join("escaped2").exists());
}
