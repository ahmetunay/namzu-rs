use serde_json::{Value, json};
use std::{
    path::Path,
    process::{Command, Output},
};
fn run(state: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_namzu-rs"))
        .args(["mcp", "--state-dir", state.to_str().unwrap()])
        .args(args)
        .output()
        .unwrap()
}
fn value(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}
fn document(server: Value) -> Value {
    json!({"version":1,"default_profile":"default","profiles":{"default":{"servers":{"fixture":server}}}})
}
fn import(state: &Path, file: &Path, server: Value) {
    std::fs::write(file, document(server).to_string()).unwrap();
    assert!(
        run(state, &["import", file.to_str().unwrap()])
            .status
            .success()
    );
}
#[test]
fn profiles_init_and_redacted_management_never_launch_server() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    assert!(run(&state, &["init"]).status.success());
    assert!(!run(&state, &["init"]).status.success());
    let file = root.path().join("selected.json");
    let marker = root.path().join("never-started");
    import(
        &state,
        &file,
        json!({"transport":{"type":"stdio","program":"/bin/sh","args":["-c",format!("touch {}",marker.display()),"PRIVATE_ARG"],"cwd":root.path()}}),
    );
    let result = run(&state, &["show", "fixture"]);
    assert!(result.status.success());
    assert_eq!(value(&result)["output"]["connect"], "ask");
    assert!(!String::from_utf8_lossy(&result.stdout).contains("PRIVATE_ARG"));
    assert!(!run(&state, &["check", "fixture"]).status.success());
    assert!(!marker.exists());
    assert!(run(&state, &["disable", "fixture"]).status.success());
    assert!(
        !run(&state, &["check", "fixture", "--approve"])
            .status
            .success()
    );
    assert!(!marker.exists());
    assert!(run(&state, &["enable", "fixture"]).status.success());
    assert!(run(&state, &["remove", "fixture"]).status.success());
    assert!(
        run(&state, &["profile", "create", "other"])
            .status
            .success()
    );
    assert!(
        run(&state, &["profile", "default", "other"])
            .status
            .success()
    );
    assert!(
        !run(&state, &["profile", "remove", "other"])
            .status
            .success()
    );
    assert!(
        run(&state, &["profile", "remove", "default"])
            .status
            .success()
    );
    assert_eq!(
        value(&run(&state, &["profile", "list"]))["output"]["profiles"],
        json!(["other"])
    );
}
#[cfg(unix)]
#[test]
fn real_stdio_check_approves_only_connect_ping_and_closes() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let file = root.path().join("selected.json");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../mcp/tests/fixtures/stdio_server.py")
        .canonicalize()
        .unwrap();
    import(
        &state,
        &file,
        json!({"transport":{"type":"stdio","program":"/usr/bin/python3","args":[fixture,"normal"],"cwd":root.path()},"policy":{"allow":["connect","ping"],"require_approval":true}}),
    );
    let result = run(&state, &["check", "fixture", "--approve"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    assert_eq!(value(&result)["output"]["model_calls"], 0);
    assert!(value(&result)["output"]["closed"].as_bool().unwrap());
    assert_eq!(
        std::fs::read_to_string(root.path().join("effects")).unwrap(),
        "1"
    );
    assert!(root.path().join("graceful").exists());
}
#[test]
fn project_config_needs_explicit_trust_and_changes_invalidate_it() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let file = project.join("project.json");
    std::fs::write(
        &file,
        document(json!({"transport":{"type":"http","url":"http://127.0.0.1:1/mcp"}})).to_string(),
    )
    .unwrap();
    let scoped = |action: &str| {
        run(
            &state,
            &[
                "--project",
                project.to_str().unwrap(),
                "--project-config",
                "project.json",
                action,
            ],
        )
    };
    assert_eq!(value(&run(&state, &["list"]))["output"], json!([]));
    assert!(!scoped("list").status.success());
    assert!(scoped("trust").status.success());
    assert_eq!(value(&scoped("list"))["output"][0]["source"], "project");
    std::fs::write(
        &file,
        format!("{} ", std::fs::read_to_string(&file).unwrap()),
    )
    .unwrap();
    assert!(!scoped("list").status.success());
    assert!(scoped("trust").status.success());
    assert!(scoped("untrust").status.success());
    assert!(!scoped("list").status.success());
}
#[test]
fn malformed_import_keeps_previous_profile_and_redacts_payload() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    assert!(run(&state, &["init"]).status.success());
    let file = root.path().join("bad.json");
    std::fs::write(&file, b"{\"SECRET_KEY\":\"DO_NOT_ECHO\"}").unwrap();
    let result = run(&state, &["import", file.to_str().unwrap()]);
    assert!(!result.status.success());
    assert_eq!(value(&result)["status"], "error");
    assert!(!String::from_utf8_lossy(&result.stdout).contains("DO_NOT_ECHO"));
    assert_eq!(
        value(&run(&state, &["profile", "list"]))["output"]["default"],
        "default"
    );
}
#[test]
fn explicit_profile_selects_only_its_own_servers_and_deny_survives_approve() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let file = root.path().join("selected.json");
    import(
        &state,
        &file,
        json!({"transport":{"type":"http","url":"http://127.0.0.1:1/mcp"},"policy":{"deny":["connect"]}}),
    );
    assert!(
        run(&state, &["profile", "create", "empty"])
            .status
            .success()
    );
    assert_eq!(
        value(&run(&state, &["--profile", "empty", "list"]))["output"],
        json!([])
    );
    let result = run(&state, &["check", "fixture", "--approve"]);
    assert!(!result.status.success());
    assert!(value(&result)["error"].as_str().unwrap().contains("denied"));
}
#[test]
fn add_update_and_cli_permission_ceiling_are_explicit() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let spec = root.path().join("server.json");
    std::fs::write(&spec,json!({"transport":{"type":"http","url":"http://127.0.0.1:1/mcp"},"policy":{"require_approval":false}}).to_string()).unwrap();
    assert!(
        run(
            &state,
            &["add", "fixture", "--spec", spec.to_str().unwrap()]
        )
        .status
        .success()
    );
    assert!(
        !run(
            &state,
            &["add", "fixture", "--spec", spec.to_str().unwrap()]
        )
        .status
        .success()
    );
    let denied = run(
        &state,
        &["--deny", "connect", "check", "fixture", "--approve"],
    );
    assert!(!denied.status.success());
    assert!(value(&denied)["error"].as_str().unwrap().contains("denied"));
    let ask = run(&state, &["--require-approval", "check", "fixture"]);
    assert!(!ask.status.success());
    assert!(value(&ask)["error"].as_str().unwrap().contains("approval"));
    assert!(
        run(
            &state,
            &["update", "fixture", "--spec", spec.to_str().unwrap()]
        )
        .status
        .success()
    );
    assert!(
        !run(
            &state,
            &["update", "missing", "--spec", spec.to_str().unwrap()]
        )
        .status
        .success()
    );
    let denied = run(
        &state,
        &["--allow", "ping", "check", "fixture", "--approve"],
    );
    assert!(value(&denied)["error"].as_str().unwrap().contains("denied"));
}
fn discovery_spec(root: &Path, mode: &str) -> Value {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../mcp/tests/fixtures/discovery_server.py")
        .canonicalize()
        .unwrap();
    json!({"transport":{"type":"stdio","program":"/usr/bin/python3","args":[fixture,mode],"cwd":root},"policy":{"require_approval":true}})
}
#[test]
fn cli_discovery_lists_all_kinds_without_executing_tools_or_auto_reading() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let file = root.path().join("spec.json");
    import(&state, &file, discovery_spec(root.path(), "normal"));
    for kind in ["tools", "resources", "prompts", "templates"] {
        let output = run(&state, &["discover", "fixture", kind, "--approve"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(
            value(&output)["output"]["count"],
            if kind == "tools" { 2 } else { 1 }
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("untrusted definition"));
    }
    let details = run(
        &state,
        &["discover", "fixture", "tools", "--approve", "--details"],
    );
    assert!(details.status.success());
    assert_eq!(
        value(&details)["output"]["items"][0]["definition"]["name"],
        "read"
    );
    let methods = std::fs::read_to_string(root.path().join("requests.jsonl")).unwrap();
    for method in ["tools/call", "resources/read", "prompts/get"] {
        assert!(!methods.contains(method));
    }
}
#[test]
fn cli_discovery_approval_deny_and_rpc_errors_do_not_publish_partial_results() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let file = root.path().join("spec.json");
    import(&state, &file, discovery_spec(root.path(), "rpc_error"));
    assert!(
        !run(&state, &["discover", "fixture", "tools"])
            .status
            .success()
    );
    assert!(!root.path().join("requests.jsonl").exists());
    let denied = run(
        &state,
        &[
            "--deny",
            "tools/list",
            "discover",
            "fixture",
            "tools",
            "--approve",
        ],
    );
    assert!(!denied.status.success());
    assert!(
        !std::fs::read_to_string(root.path().join("requests.jsonl"))
            .unwrap()
            .contains("tools/list")
    );
    let failed = run(&state, &["discover", "fixture", "tools", "--approve"]);
    assert!(!failed.status.success());
    assert_eq!(value(&failed)["status"], "error");
    assert!(!String::from_utf8_lossy(&failed.stdout).contains("DO_NOT_LOG_REMOTE_SECRET"));
    assert!(value(&failed).get("output").is_none());
}
