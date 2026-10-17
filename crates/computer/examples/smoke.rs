//! Explicit real-Docker acceptance; ordinary cargo tests never require Docker.
use namzu_computer::{
    Answer, ComputerManager, ComputerRecord, ComputerSpec, Docker, Engine, GuestPolicy, GuestTools,
    Operation, Phase,
    protocol::{FILE_LIMIT, GuestRequest, GuestValue},
};
use namzu_core::{
    AgentError, Cancellation, Completion, Limits, ScriptedProvider, ToolCall, TurnContext, run_turn,
};
use namzu_session::PrivateStore;
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::Path,
    process::{Child, Command},
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn context() -> TurnContext {
    TurnContext::new(Duration::from_secs(300), Cancellation::default()).unwrap()
}
fn manager(path: &Path, pal: &str) -> Result<ComputerManager> {
    Ok(ComputerManager::from_store(
        PrivateStore::open(path)?,
        "acceptance",
        pal,
        Docker::local("/usr/local/bin/docker")?,
    )?)
}
fn shell(computer: &mut namzu_computer::Computer<'_>, command: &str) -> Result<String> {
    let result = computer.shell(command, ".", Duration::from_secs(15), &context())?;
    if result.status != "exited" || result.exit_code != Some(0) {
        return Err(format!("acceptance command failed: {}", result.status).into());
    }
    Ok(result.stdout)
}
struct Cleanup<'a>(&'a ComputerManager);
impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        let _ = self.0.recover_cleanup(&context());
    }
}
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn marker(record: &ComputerRecord, path: &str) -> Result<bool> {
    let mut engine = Docker::local("/usr/local/bin/docker")?;
    let Answer::Container(Some(info)) =
        engine.perform(Operation::Inspect(record.container.clone()), &context())?
    else {
        return Ok(false);
    };
    let Answer::Guest(reply) = engine.perform(
        Operation::Exec {
            container: info.id.clone(),
            request: GuestRequest::Read { path: path.into() },
        },
        &context(),
    )?
    else {
        return Ok(false);
    };
    Ok(matches!(reply.result, Ok(GuestValue::Bytes { .. })))
}
fn gone(record: &ComputerRecord) -> Result<()> {
    let mut engine = Docker::local("/usr/local/bin/docker")?;
    assert!(matches!(
        engine.perform(Operation::Inspect(record.container.clone()), &context())?,
        Answer::Container(None)
    ));
    assert!(matches!(
        engine.perform(Operation::VolumeInspect(record.volume.clone()), &context())?,
        Answer::Volume(None)
    ));
    Ok(())
}
fn acceptance(
    image: &str,
    a: &ComputerManager,
    b: &ComputerManager,
    crash_root: &Path,
) -> Result<BTreeMap<&'static str, &'static str>> {
    let mut checks = BTreeMap::new();
    let ctx = context();
    let mut first = a.acquire(ComputerSpec::new(image), &ctx)?;
    let mut second = b.acquire(ComputerSpec::new(image), &ctx)?;
    first.upload("private.bin", &[0, 255, 10], &ctx)?;
    assert!(second.download("private.bin", &ctx).is_err());
    checks.insert("pal_workspace_isolation", "passed");
    let bytes = (0..FILE_LIMIT).map(|i| (i % 256) as u8).collect::<Vec<_>>();
    first.upload("limit.bin", &bytes, &ctx)?;
    assert_eq!(first.download("limit.bin", &ctx)?, bytes);
    assert!(
        first
            .upload("too-big", &vec![0; FILE_LIMIT + 1], &ctx)
            .is_err()
    );
    checks.insert("bounded_binary_transfer", "passed");
    shell(
        &mut first,
        "test -z \"$NAMZU_COMPUTER_SENTINEL\" && test ! -S /var/run/docker.sock && test \"$(ls /sys/class/net)\" = lo && grep -q '^CapEff:[[:space:]]*0000000000000000$' /proc/self/status && grep -q '^NoNewPrivs:[[:space:]]*1$' /proc/self/status && test ! -e /workspace/HOST-ONLY",
    )?;
    checks.insert("network_caps_credentials_mount_isolation", "passed");
    let host_marker = crash_root
        .parent()
        .ok_or("fixture parent required")?
        .join("HOST-ONLY");
    assert!(host_marker.exists());
    shell(
        &mut first,
        &format!(
            "test ! -e '{}'",
            host_marker.to_string_lossy().replace('\'', "'\\''")
        ),
    )?;
    checks.insert("host_state_not_mounted", "passed");
    let limits = shell(
        &mut first,
        "cat /sys/fs/cgroup/memory.max /sys/fs/cgroup/pids.max /sys/fs/cgroup/cpu.max",
    )?;
    let lines = limits.lines().collect::<Vec<_>>();
    assert_eq!(lines[0], "268435456");
    assert_eq!(lines[1], "64");
    assert_eq!(lines[2], "100000 100000");
    checks.insert("effective_memory_cpu_pid_limits", "passed");
    shell(
        &mut first,
        "if touch /etc/namzu-write-probe 2>/dev/null; then exit 9; fi",
    )?;
    checks.insert("readonly_root", "passed");
    shell(&mut first, "ln -s /etc/passwd escape; mkfifo pipe")?;
    assert!(first.download("escape", &ctx).is_err());
    assert!(first.download("pipe", &ctx).is_err());
    assert!(first.download("../outside", &ctx).is_err());
    first.upload("escape", b"replacement", &ctx)?;
    assert_eq!(first.download("escape", &ctx)?, b"replacement");
    checks.insert("symlink_fifo_traversal_boundaries", "passed");
    shell(
        &mut first,
        "setsid sh -c 'echo $$ > escaped.pid; exec sleep 600' >/dev/null 2>&1 & while [ ! -s escaped.pid ]; do :; done",
    )?;
    shell(
        &mut first,
        "if kill -0 \"$(cat escaped.pid)\" 2>/dev/null; then exit 9; fi",
    )?;
    checks.insert("escaped_background_descendant_cleanup", "passed");
    first.restart(&ctx)?;
    assert_eq!(first.download("private.bin", &ctx)?, [0, 255, 10]);
    checks.insert("restart_keeps_files_without_command_replay", "passed");
    let timed = first.shell(
        "printf 'once\\n' >> timeout-audit; sleep 600",
        ".",
        Duration::from_millis(200),
        &ctx,
    );
    assert!(timed.is_err() || timed.is_ok_and(|o| o.status == "timeout"));
    assert_eq!(first.snapshot().phase, Phase::Stopped);
    first.start(&ctx)?;
    assert_eq!(first.download("timeout-audit", &ctx)?, b"once\n");
    if first.snapshot().pending.is_some() {
        assert!(first.upload("blocked", b"no", &ctx).is_err());
        first.acknowledge_uncertain()?;
    }
    checks.insert("timeout_stops_computer_without_replay", "passed");
    let cancel = TurnContext::new(Duration::from_secs(30), Cancellation::default())?;
    let token = cancel.cancellation();
    let record = first.snapshot().clone();
    std::thread::scope(|scope| -> Result<()> {
        let waiter = scope.spawn(move || -> std::result::Result<(), String> {
            let end = Instant::now() + Duration::from_secs(15);
            loop {
                match marker(&record, "cancel-ready") {
                    Ok(true) => {
                        token.cancel();
                        return Ok(());
                    }
                    Err(e) => {
                        token.cancel();
                        return Err(e.to_string());
                    }
                    _ => {}
                }
                if Instant::now() >= end {
                    token.cancel();
                    return Err("guest readiness timeout".into());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let result = first.shell(
            "printf ready > cancel-ready; sleep 600",
            ".",
            Duration::from_secs(30),
            &cancel,
        );
        waiter.join().expect("readiness thread panicked")?;
        assert_eq!(result.unwrap_err(), AgentError::Cancelled);
        Ok(())
    })?;
    assert_eq!(first.snapshot().phase, Phase::Stopped);
    assert!(first.snapshot().pending.is_some());
    first.start(&ctx)?;
    assert_eq!(first.download("cancel-ready", &ctx)?, b"ready");
    first.acknowledge_uncertain()?;
    checks.insert("live_cancellation_kills_remote_work", "passed");
    let flooded = first.shell("yes x", ".", Duration::from_secs(10), &ctx)?;
    assert_eq!(flooded.status, "output_limit");
    assert_eq!(first.snapshot().phase, Phase::Stopped);
    first.start(&ctx)?;
    checks.insert("output_limit_stops_computer", "passed");
    let a_record = first.snapshot().clone();
    first.close(&ctx)?;
    drop(first);
    gone(&a_record)?;
    let b_record = second.snapshot().clone();
    let mut tools = GuestTools::new(
        second,
        GuestPolicy {
            allow_write: true,
            ..GuestPolicy::default()
        },
    );
    let mut provider = ScriptedProvider(
        vec![
            Completion {
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "w".into(),
                    name: "write_file".into(),
                    arguments: json!({"path":"sdk.txt","content":"12"}),
                }],
            },
            Completion {
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "r".into(),
                    name: "read_file".into(),
                    arguments: json!({"path":"sdk.txt"}),
                }],
            },
            Completion {
                text: "12".into(),
                tool_calls: vec![],
            },
        ]
        .into(),
    );
    let turn = run_turn(
        &mut provider,
        &mut tools,
        "offline guest SDK task",
        Limits::default(),
        Cancellation::default(),
    )?;
    assert_eq!(turn.output, "12");
    assert_eq!(turn.stats.tool_calls, 2);
    tools.computer_mut().close(&ctx)?;
    drop(tools);
    gone(&b_record)?;
    checks.insert("sdk_uses_guest_tools", "passed");
    checks.insert("owned_container_and_volume_cleanup", "passed");
    let mut child = ChildGuard(
        Command::new(std::env::current_exe()?)
            .arg(image)
            .arg("--crash-child")
            .arg(crash_root)
            .spawn()?,
    );
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(record) = a.snapshot()?
            && record.pending.as_ref().is_some_and(|p| p.kind == "shell")
            && marker(&record, "crash-audit")?
        {
            break;
        }
        if Instant::now() >= end {
            return Err("crash fixture readiness timeout".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.0.kill()?;
    child.0.wait()?;
    let mut recovered = a.resume(&ctx)?;
    assert_eq!(recovered.snapshot().phase, Phase::Stopped);
    assert!(recovered.snapshot().pending.is_some());
    recovered.start(&ctx)?;
    assert_eq!(recovered.download("crash-audit", &ctx)?, b"once\n");
    assert!(recovered.upload("blocked", b"no", &ctx).is_err());
    recovered.acknowledge_uncertain()?;
    let record = recovered.snapshot().clone();
    recovered.close(&ctx)?;
    drop(recovered);
    gone(&record)?;
    checks.insert(
        "real_client_crash_stops_and_preserves_uncertain_effect",
        "passed",
    );
    Ok(checks)
}
fn main() -> Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    let image = args.get(1).ok_or("pinned local image ID required")?;
    if args.get(2).is_some_and(|s| s == "--crash-child") {
        let manager = manager(
            Path::new(args.get(3).ok_or("fixture root required")?),
            "pal-a",
        )?;
        let ctx = context();
        let mut computer = manager.acquire(ComputerSpec::new(image), &ctx)?;
        computer.shell(
            "printf 'once\\n' >> crash-audit; sleep 600",
            ".",
            Duration::from_secs(300),
            &ctx,
        )?;
        return Err("fixture should be killed while guest is running".into());
    }
    let root = tempfile::tempdir()?;
    let a_root = root.path().join("a");
    let a = manager(&a_root, "pal-a")?;
    let b = manager(&root.path().join("b"), "pal-b")?;
    std::fs::write(root.path().join("HOST-ONLY"), b"host fixture")?;
    let _cleanup_a = Cleanup(&a);
    let _cleanup_b = Cleanup(&b);
    let checks = acceptance(image, &a, &b, &a_root)?;
    println!(
        "{}",
        json!({"real_docker":true,"model_calls":0,"checks":checks,"cleanup":"passed"})
    );
    Ok(())
}
