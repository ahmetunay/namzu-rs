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
        check_timeout_seconds: 30,
        check_output_limit: 32768,
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

#[test]
fn file_gates_distinguish_missing_mismatch_directory_symlink_and_oversize() {
    let root = tempfile::tempdir().unwrap();
    let mut spec = suite();
    spec.agents = vec![
        agent("missing", "true"),
        agent("mismatch", "printf wrong > answer.txt"),
        agent("directory", "mkdir answer.txt"),
        agent(
            "symlink",
            "printf '12\\n' > actual; ln -s actual answer.txt",
        ),
        agent("large", "head -c 65537 /dev/zero > answer.txt"),
    ];
    let report = run(&spec, root.path(), Cancellation::default(), |_| {}).unwrap();
    for (row, outcome) in report["runs"].as_array().unwrap().iter().zip([
        "missing",
        "mismatch",
        "unsafe_path",
        "unsafe_path",
        "too_large",
    ]) {
        assert_eq!(row["status"], "incorrect");
        assert_eq!(row["oracle"]["files"][0]["outcome"], outcome);
    }
}

#[test]
fn every_gate_runs_and_checker_diagnostics_stay_in_private_log() {
    let root = tempfile::tempdir().unwrap();
    let mut spec = suite();
    spec.agents = vec![agent("noop", "true")];
    spec.tasks[0].check = vec![
        "/bin/sh".into(),
        "-c".into(),
        "touch checked; printf private-checker-diagnostic; exit 1".into(),
    ];
    let report = run(&spec, root.path(), Cancellation::default(), |_| {}).unwrap();
    let row = &report["runs"][0];
    assert_eq!(row["oracle"]["files"][0]["outcome"], "missing");
    assert_eq!(row["oracle"]["check"]["outcome"], "test_failed");
    assert!(!report.to_string().contains("private-checker-diagnostic"));
    let out = PathBuf::from(report["output_directory"].as_str().unwrap());
    assert!(out.join("run-0-workspace/checked").exists());
    let log = std::fs::read_to_string(out.join("run-0.log.json")).unwrap();
    assert!(log.contains("private-checker-diagnostic"));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(out.join("run-0.log.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn checker_timeout_output_limit_and_crash_are_unavailable_not_wrong_answers() {
    for (command, timeout, limit, outcome) in [
        ("sleep 60", 1, 32768, "timeout"),
        ("head -c 65536 /dev/zero", 30, 32, "output_limit"),
        ("kill -KILL $$", 30, 32768, "error"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut spec = suite();
        spec.agents.truncate(1);
        spec.tasks[0].check = vec!["/bin/sh".into(), "-c".into(), command.into()];
        spec.tasks[0].check_timeout_seconds = timeout;
        spec.tasks[0].check_output_limit = limit;
        let report = run(&spec, root.path(), Cancellation::default(), |_| {}).unwrap();
        assert_eq!(report["runs"][0]["status"], "oracle_error");
        assert!(report["runs"][0]["oracle_passed"].is_null());
        assert_eq!(report["runs"][0]["oracle"]["check"]["outcome"], outcome);
        assert_eq!(report["summary"][0]["oracle_errors"], 1);
        assert_eq!(report["summary"][0]["incorrect"], 0);
    }
}

#[test]
fn all_commands_are_resolved_before_any_agent_or_artifact_effect() {
    let root = tempfile::tempdir().unwrap();
    let mut spec = suite();
    spec.tasks.push(Task {
        id: "invalid-checker".into(),
        check: vec!["/no/such/checker".into()],
        ..task()
    });
    assert!(run(&spec, root.path(), Cancellation::default(), |_| {}).is_err());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn invalid_checker_budgets_argv_and_conflicting_inputs_fail_admission() {
    let mut spec = suite();
    spec.tasks[0].check_timeout_seconds = 31;
    assert!(spec.validate().is_err());
    spec.tasks[0].check_timeout_seconds = 0;
    assert!(spec.validate().is_err());
    spec.tasks[0].check_timeout_seconds = 30;
    spec.tasks[0].check_output_limit = 32769;
    assert!(spec.validate().is_err());
    spec.tasks[0].check_output_limit = 32768;
    spec.tasks[0].check = vec!["".into()];
    assert!(spec.validate().is_err());
    spec.tasks[0].check = vec!["/bin/sh".into(), "nul\0arg".into()];
    assert!(spec.validate().is_err());
    spec.tasks[0].check.clear();
    spec.tasks[0].files.insert("a".into(), "file".into());
    spec.tasks[0].files.insert("a/b".into(), "nested".into());
    assert!(spec.validate().is_err());
}

#[test]
fn legacy_suite_defaults_remain_compatible_but_unknown_fields_are_rejected() {
    let mut value = serde_json::to_value(suite()).unwrap();
    for task in value["tasks"].as_array_mut().unwrap() {
        task.as_object_mut()
            .unwrap()
            .remove("check_timeout_seconds");
        task.as_object_mut().unwrap().remove("check_output_limit");
    }
    let spec: Suite = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(spec.tasks[0].check_timeout_seconds, 30);
    assert_eq!(spec.tasks[0].check_output_limit, 32768);
    value["tasks"][0]["typo"] = serde_json::json!(true);
    assert!(serde_json::from_value::<Suite>(value).is_err());
}

#[test]
fn oracle_identity_changes_with_expected_data_and_limits_not_agent() {
    let root = tempfile::tempdir().unwrap();
    let mut spec = suite();
    let a = run(&spec, root.path(), Cancellation::default(), |_| {}).unwrap();
    assert_eq!(a["runs"][0]["oracle_sha256"], a["runs"][1]["oracle_sha256"]);
    spec.tasks[0]
        .expected_files
        .insert("answer.txt".into(), "13\n".into());
    let b = run(&spec, root.path(), Cancellation::default(), |_| {}).unwrap();
    assert_ne!(a["runs"][0]["oracle_sha256"], b["runs"][0]["oracle_sha256"]);
    assert_eq!(a["runs"][0]["prompt_sha256"], b["runs"][0]["prompt_sha256"]);
    assert_eq!(a["runs"][0]["input_sha256"], b["runs"][0]["input_sha256"]);
}

#[test]
fn cancellation_keeps_completed_rows_and_starts_no_following_agent() {
    let root = tempfile::tempdir().unwrap();
    let cancellation = Cancellation::default();
    let mut completed = 0;
    let result = run(&suite(), root.path(), cancellation.clone(), |_| {
        completed += 1;
        cancellation.cancel();
    });
    assert_eq!(result.unwrap_err(), namzu_core::AgentError::Cancelled);
    assert_eq!(completed, 1);
    let out = std::fs::read_dir(root.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.join("report.json")).unwrap()).unwrap();
    assert_eq!(report["runs"].as_array().unwrap().len(), 1);
    assert!(!out.join("run-1-workspace").exists());
}

#[test]
fn all_builtin_oracles_accept_known_solutions_and_reject_seeded_bugs() {
    let root = tempfile::tempdir().unwrap();
    let python = executable(std::path::Path::new("python3")).unwrap();
    for id in BUILTIN_TASKS {
        let mut spec = suite();
        spec.tasks = vec![builtin_task(id).unwrap()];
        spec.agents = vec![Agent {
            program: python.clone(), args: vec!["-I".into(), "-c".into(),
                "from pathlib import Path; import sys; Path('solution.py').write_text(sys.argv[1])".into(),
                fixture_solution(id).unwrap().into()], ..agent("correct", "")
        }, agent("noop", "printf done")];
        let report = run(&spec, root.path(), Cancellation::default(), |_| {}).unwrap();
        assert_eq!(report["runs"][0]["status"], "passed", "{id}: {report}");
        assert_eq!(report["runs"][1]["status"], "incorrect", "{id}: {report}");
    }
}

#[test]
fn refactor_cannot_change_protected_file_even_when_behavior_passes() {
    let root = tempfile::tempdir().unwrap();
    let mut spec = suite();
    spec.tasks = vec![builtin_task("refactor").unwrap()];
    spec.agents = vec![Agent {
        program: executable(std::path::Path::new("python3")).unwrap(),
        args: vec!["-I".into(), "-c".into(),
            "from pathlib import Path; import sys; Path('solution.py').write_text(sys.argv[1]); p=Path('legacy.py'); p.write_text(p.read_text()+'\\n# tamper\\n')".into(),
            fixture_solution("refactor").unwrap().into()], ..agent("tamper", "")
    }];
    let report = run(&spec, root.path(), Cancellation::default(), |_| {}).unwrap();
    assert_eq!(report["runs"][0]["status"], "incorrect");
    assert_eq!(
        report["runs"][0]["oracle"]["files"][0]["outcome"],
        "mismatch"
    );
    assert_eq!(report["runs"][0]["oracle"]["check"]["outcome"], "passed");
}

#[test]
fn agent_timeout_output_overflow_crash_and_launch_errors_remain_execution_failures() {
    let root = tempfile::tempdir().unwrap();
    let bad_program = root.path().join("unlaunchable");
    std::fs::write(&bad_program, "#!/definitely/missing/interpreter\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bad_program, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut spec = suite();
    spec.timeout_seconds = 1;
    spec.agents = vec![
        agent("timeout", "sleep 60"),
        agent("overflow", "head -c 2097152 /dev/zero"),
        agent("crash", "kill -KILL $$"),
        Agent {
            program: bad_program,
            ..agent("launch-error", "")
        },
    ];
    let report = run(
        &spec,
        &root.path().join("reports"),
        Cancellation::default(),
        |_| {},
    )
    .unwrap();
    for (row, expected) in report["runs"].as_array().unwrap().iter().zip([
        "timeout",
        "output_limit",
        "agent_error",
        "agent_error",
    ]) {
        assert_eq!(row["status"], expected);
    }
    assert_eq!(report["status"], "complete");
    assert_eq!(report["runs"][3]["execution_status"], "error");
    assert!(report["runs"][3]["peak_rss_kib"].is_null());
    assert!(
        report["summary"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["execution_errors"] == 1)
    );
}

#[test]
fn public_oracle_sdk_rejects_empty_or_escaping_gates() {
    let root = tempfile::tempdir().unwrap();
    let mut task = task();
    task.expected_files.clear();
    assert!(oracle::evaluate(&task, root.path(), Cancellation::default()).is_err());
    task.expected_files.insert("../outside".into(), "x".into());
    assert!(oracle::evaluate(&task, root.path(), Cancellation::default()).is_err());
}

#[test]
fn checker_launch_failure_is_inconclusive_and_does_not_abort_other_runs() {
    let root = tempfile::tempdir().unwrap();
    let bad_program = root.path().join("unlaunchable");
    std::fs::write(&bad_program, "not executable").unwrap();
    let mut spec = suite();
    spec.tasks[0].check = vec![bad_program.to_str().unwrap().into()];
    let report = run(
        &spec,
        &root.path().join("reports"),
        Cancellation::default(),
        |_| {},
    )
    .unwrap();
    assert_eq!(report["runs"].as_array().unwrap().len(), 2);
    assert!(
        report["runs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["status"] == "oracle_error")
    );
    assert_eq!(report["runs"][0]["oracle"]["check"]["outcome"], "error");
}
