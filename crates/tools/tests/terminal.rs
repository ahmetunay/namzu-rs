use namzu_core::{Cancellation, ToolCall, Tools, TurnContext};
use namzu_tools::{TerminalTools, ToolPolicy};
use serde_json::{Value, json};
use std::time::Duration;

fn invoke(
    root: &std::path::Path,
    policy: ToolPolicy,
    name: &str,
    args: Value,
) -> namzu_core::ToolResult {
    let mut tools = TerminalTools::new(root, policy).unwrap();
    tools
        .call(
            &ToolCall {
                id: "test".into(),
                name: name.into(),
                arguments: args,
            },
            &TurnContext::new(Duration::from_secs(10), Cancellation::default()).unwrap(),
        )
        .unwrap()
}

#[test]
fn shell_is_absent_and_denied_without_explicit_grant() {
    let root = tempfile::tempdir().unwrap();
    let tools = TerminalTools::new(root.path(), ToolPolicy::default()).unwrap();
    assert!(!tools.definitions().iter().any(|d| d.name == "shell"));
    assert!(
        invoke(
            root.path(),
            ToolPolicy::default(),
            "shell",
            json!({"command":"touch escaped"})
        )
        .is_error
    );
    assert!(!root.path().join("escaped").exists());
}

#[test]
fn deny_beats_allow_even_for_direct_dispatch() {
    let root = tempfile::tempdir().unwrap();
    let policy = ToolPolicy {
        allow_shell: true,
        allow: Some(["shell".into()].into()),
        deny: ["shell".into()].into(),
        ..ToolPolicy::default()
    };
    assert!(
        TerminalTools::new(root.path(), policy.clone())
            .unwrap()
            .definitions()
            .is_empty()
    );
    assert!(
        invoke(
            root.path(),
            policy,
            "shell",
            json!({"command":"touch escaped"})
        )
        .is_error
    );
    assert!(!root.path().join("escaped").exists());
}

#[test]
fn shell_preserves_exit_status_stdout_and_stderr() {
    let root = tempfile::tempdir().unwrap();
    let output = invoke(
        root.path(),
        ToolPolicy {
            allow_shell: true,
            ..ToolPolicy::default()
        },
        "shell",
        json!({"command":"printf out; printf err >&2; exit 7"}),
    );
    assert!(output.is_error);
    let value: Value = serde_json::from_str(&output.content).unwrap();
    assert_eq!(value["stdout"], "out");
    assert_eq!(value["stderr"], "err");
    assert_eq!(value["exit_code"], 7);
}

#[test]
fn shell_output_is_bounded_and_writer_terminated() {
    let root = tempfile::tempdir().unwrap();
    let output = invoke(
        root.path(),
        ToolPolicy {
            allow_shell: true,
            ..ToolPolicy::default()
        },
        "shell",
        json!({"command":"yes x"}),
    );
    assert!(output.is_error);
    let value: Value = serde_json::from_str(&output.content).unwrap();
    assert_eq!(value["status"], "output_limit");
    assert!(
        value["stdout"].as_str().unwrap().len() + value["stderr"].as_str().unwrap().len() <= 65536
    );
}

#[test]
fn shell_cwd_is_validated_before_effects() {
    let root = tempfile::tempdir().unwrap();
    for cwd in ["../", "/tmp", "missing"] {
        assert!(
            invoke(
                root.path(),
                ToolPolicy {
                    allow_shell: true,
                    ..ToolPolicy::default()
                },
                "shell",
                json!({"command":"touch bad","cwd":cwd})
            )
            .is_error
        );
    }
}

#[test]
fn search_is_sorted_literal_and_skips_symlinks() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("b.txt"), "needle\n").unwrap();
    std::fs::write(root.path().join("a.txt"), "x\nneedle .*\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc/passwd", root.path().join("escape")).unwrap();
    let output = invoke(
        root.path(),
        ToolPolicy::default(),
        "search_files",
        json!({"query":"needle"}),
    );
    assert!(!output.is_error);
    let value: Value = serde_json::from_str(&output.content).unwrap();
    assert_eq!(value["matches"].as_array().unwrap().len(), 2);
    assert_eq!(value["matches"][0]["path"], "./a.txt");
    assert_eq!(value["matches"][0]["line"], 2);
    let value: Value = serde_json::from_str(
        &invoke(
            root.path(),
            ToolPolicy::default(),
            "search_files",
            json!({"query":".*"}),
        )
        .content,
    )
    .unwrap();
    assert_eq!(value["matches"].as_array().unwrap().len(), 1);
}

#[test]
fn search_refuses_escape_and_bounds_match_count() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("many"), "needle\n".repeat(100)).unwrap();
    assert!(
        invoke(
            root.path(),
            ToolPolicy::default(),
            "search_files",
            json!({"path":"..","query":"needle"})
        )
        .is_error
    );
    let value: Value = serde_json::from_str(
        &invoke(
            root.path(),
            ToolPolicy::default(),
            "search_files",
            json!({"query":"needle"}),
        )
        .content,
    )
    .unwrap();
    assert_eq!(value["truncated"], true);
    assert_eq!(value["matches"].as_array().unwrap().len(), 64);
}

#[test]
fn patch_requires_unique_exact_match_and_emits_diff() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("x");
    std::fs::write(&path, "one\ntwo\n").unwrap();
    let policy = ToolPolicy {
        allow_write: true,
        ..ToolPolicy::default()
    };
    let result = invoke(
        root.path(),
        policy.clone(),
        "apply_patch",
        json!({"path":"x","old_text":"two","new_text":"three"}),
    );
    assert!(!result.is_error);
    assert!(result.content.contains("-two\n+three"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\nthree\n");
    assert!(
        invoke(
            root.path(),
            policy.clone(),
            "apply_patch",
            json!({"path":"x","old_text":"missing","new_text":"bad"})
        )
        .is_error
    );
    std::fs::write(&path, "same same").unwrap();
    assert!(
        invoke(
            root.path(),
            policy,
            "apply_patch",
            json!({"path":"x","old_text":"same","new_text":"bad"})
        )
        .is_error
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), "same same");
}

#[test]
fn readonly_diff_changes_nothing_and_patch_is_denied() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("x");
    std::fs::write(&path, "old\n").unwrap();
    assert!(
        !invoke(
            root.path(),
            ToolPolicy::default(),
            "diff_file",
            json!({"path":"x","content":"new\n"})
        )
        .is_error
    );
    assert!(
        invoke(
            root.path(),
            ToolPolicy::default(),
            "apply_patch",
            json!({"path":"x","old_text":"old","new_text":"new"})
        )
        .is_error
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), "old\n");
}

#[cfg(unix)]
#[test]
fn patch_preserves_executable_mode() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("x");
    std::fs::write(&path, "old\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o750)).unwrap();
    assert!(
        !invoke(
            root.path(),
            ToolPolicy {
                allow_write: true,
                ..ToolPolicy::default()
            },
            "apply_patch",
            json!({"path":"x","old_text":"old","new_text":"new"})
        )
        .is_error
    );
    assert_eq!(std::fs::metadata(path).unwrap().mode() & 0o777, 0o750);
}

#[cfg(unix)]
#[test]
fn cancellation_after_child_handshake_kills_process_group() {
    use std::io::Read;
    let root = tempfile::tempdir().unwrap();
    let fifo = root.path().join("ready");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let cancellation = Cancellation::default();
    let signal = cancellation.clone();
    let cwd = root.path().to_path_buf();
    let worker = std::thread::spawn(move || {
        let mut tools = TerminalTools::new(
            cwd,
            ToolPolicy {
                allow_shell: true,
                ..ToolPolicy::default()
            },
        )
        .unwrap();
        tools.call(&ToolCall {id:"test".into(),name:"shell".into(),arguments:json!({"command":"sleep 60 & echo $! > child.pid; printf ready > ready; wait"})},&TurnContext::new(Duration::from_secs(30),signal).unwrap())
    });
    let mut ready = String::new();
    std::fs::File::open(fifo)
        .unwrap()
        .read_to_string(&mut ready)
        .unwrap();
    assert_eq!(ready, "ready");
    let pid = std::fs::read_to_string(root.path().join("child.pid"))
        .unwrap()
        .trim()
        .parse::<u32>()
        .unwrap();
    #[cfg(target_os = "linux")]
    let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) as i32 };
    #[cfg(target_os = "linux")]
    assert!(pidfd >= 0);
    cancellation.cancel();
    assert_eq!(
        worker.join().unwrap().unwrap_err(),
        namzu_core::AgentError::Cancelled
    );
    // Await the kernel exit event, not a wall-clock race against signal delivery.
    #[cfg(target_os = "linux")]
    unsafe {
        let mut event = libc::pollfd {
            fd: pidfd,
            events: libc::POLLIN,
            revents: 0,
        };
        loop {
            let result = libc::poll(&mut event, 1, -1);
            if result >= 0 {
                assert!(result > 0);
                break;
            }
            assert_eq!(
                std::io::Error::last_os_error().kind(),
                std::io::ErrorKind::Interrupted
            );
        }
        libc::close(pidfd);
    }
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        assert!(
            stat.rsplit_once(')')
                .unwrap()
                .1
                .trim_start()
                .starts_with('Z')
        );
    }
}
