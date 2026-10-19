//! Explicit real-Docker acceptance, including kernel-enforced failure boundaries.
use namzu_computer::{
    Answer, ComputerManager, ComputerSpec, Docker, Engine, GuestPolicy, GuestTools, Operation,
    Phase,
};
use namzu_core::{Cancellation, Tools, TurnContext};
use namzu_sandbox::Policy;
use namzu_session::PrivateStore;
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn ctx() -> TurnContext {
    TurnContext::new(Duration::from_secs(300), Cancellation::default()).unwrap()
}
fn probe(name: &str) -> Result<()> {
    match name {
        "file" => {
            unsafe {
                libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
            }
            let mut f = std::fs::File::create("large")?;
            assert!(f.write_all(&[0; 8192]).is_err());
            assert!(f.metadata()?.len() <= 4096);
        }
        "fds" => {
            let mut files = Vec::new();
            loop {
                match std::fs::File::open("/dev/null") {
                    Ok(f) => {
                        files.push(f);
                        assert!(files.len() < 32);
                    }
                    Err(e) => {
                        assert_eq!(e.raw_os_error(), Some(libc::EMFILE));
                        break;
                    }
                }
            }
        }
        "pids" => {
            let mut children = Vec::new();
            loop {
                match Command::new("/bin/sleep")
                    .arg("30")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                {
                    Ok(child) => {
                        children.push(child);
                        if children.len() >= 32 {
                            break;
                        }
                    }
                    Err(e) => {
                        assert_eq!(e.raw_os_error(), Some(libc::EAGAIN));
                        break;
                    }
                }
            }
            let count = children.len();
            for mut child in children {
                let _ = child.kill();
                let _ = child.wait();
            }
            assert!(count > 0 && count < 16);
        }
        "scratch" => {
            let mut exhausted = false;
            for i in 0..512 {
                let path = format!("/tmp/fill-{i}");
                match std::fs::write(&path, [0; 4096]) {
                    Ok(()) => (),
                    Err(e) => {
                        assert_eq!(e.raw_os_error(), Some(libc::ENOSPC));
                        exhausted = true;
                        break;
                    }
                }
            }
            assert!(exhausted);
        }
        "network" => {
            let port = std::env::args().nth(3).unwrap();
            let addr = format!("127.0.0.1:{port}").parse()?;
            assert!(std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_err());
            let route = std::fs::read_to_string("/proc/net/route")?;
            assert_eq!(route.lines().count(), 1);
        }
        "noexec" => {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write("/tmp/blocked", b"#!/bin/sh\nexit 0\n")?;
            std::fs::set_permissions("/tmp/blocked", std::fs::Permissions::from_mode(0o700))?;
            assert_eq!(
                Command::new("/tmp/blocked")
                    .status()
                    .unwrap_err()
                    .raw_os_error(),
                Some(libc::EACCES)
            );
        }
        _ => return Err("unknown probe".into()),
    }
    println!("passed {name}");
    Ok(())
}
fn manager(path: &Path, id: &str) -> Result<ComputerManager> {
    Ok(ComputerManager::from_store(
        PrivateStore::open(path.join(id))?,
        "sandbox-test",
        id,
        Docker::local("/usr/local/bin/docker")?,
    )?)
}
fn command(c: &mut namzu_computer::Computer<'_>, s: &str) -> Result<String> {
    let o = c.shell(s, ".", Duration::from_secs(30), &ctx())?;
    assert_eq!(o.status, "exited", "{s}: {o:?}");
    assert_eq!(o.exit_code, Some(0), "{s}: {o:?}");
    Ok(o.stdout)
}
fn gone(record: &namzu_computer::ComputerRecord) -> Result<()> {
    let mut docker = Docker::local("/usr/local/bin/docker")?;
    assert!(matches!(
        docker.perform(Operation::Inspect(record.container.clone()), &ctx())?,
        Answer::Container(None)
    ));
    assert!(matches!(
        docker.perform(Operation::VolumeInspect(record.volume.clone()), &ctx())?,
        Answer::Volume(None)
    ));
    Ok(())
}
struct Cleanup<'a>(&'a ComputerManager);
impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        let _ = self.0.recover_cleanup(&ctx());
    }
}
fn main() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("probe") {
        return probe(&std::env::args().nth(2).unwrap());
    }
    let image = std::env::args().nth(1).ok_or("image ID required")?;
    let temp = tempfile::tempdir()?;
    let mut checks = Vec::new();
    let mut p = Policy::build();
    p.limits.file_bytes = 4096;
    p.limits.open_files = 32;
    p.limits.pids = 16;
    p.limits.scratch_bytes = 1024 * 1024;
    p.limits.output_bytes = 1024;
    {
        let manager = manager(temp.path(), "rw")?;
        let _cleanup = Cleanup(&manager);
        let mut c = manager.acquire(ComputerSpec::new(&image).with_sandbox(p.clone())?, &ctx())?;
        c.upload("ok", b"hello", &ctx())?;
        assert_eq!(c.download("ok", &ctx())?, b"hello");
        assert!(c.upload("too-big", &vec![0; 4097], &ctx()).is_err());
        checks.push("sdk_rw_and_file_preflight");
        for name in ["file", "fds", "pids", "noexec"] {
            assert!(
                command(
                    &mut c,
                    &format!("/usr/local/bin/sandbox-probe probe {name}")
                )?
                .contains("passed")
            );
            checks.push(name);
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        command(
            &mut c,
            &format!(
                "/usr/local/bin/sandbox-probe probe network {}",
                listener.local_addr()?.port()
            ),
        )?;
        checks.push("host_loopback_and_routes_isolated");
        assert_eq!(
            command(
                &mut c,
                "if touch /etc/forbidden 2>/dev/null; then exit 1; fi; echo root-readonly"
            )?
            .trim(),
            "root-readonly"
        );
        checks.push("root_readonly");
        // Fill scratch last, so earlier subprocess supervision retains its working scratch.
        command(&mut c, "/usr/local/bin/sandbox-probe probe scratch")?;
        checks.push("scratch_quota");
        let record = c.snapshot().clone();
        c.close(&ctx())?;
        gone(&record)?;
        checks.push("healthy_resources_removed");
    }
    {
        let manager = manager(temp.path(), "ro")?;
        let _cleanup = Cleanup(&manager);
        let mut ro = Policy::readonly();
        ro.limits.output_bytes = 1024;
        let mut c = manager.acquire(ComputerSpec::new(&image).with_sandbox(ro)?, &ctx())?;
        assert_eq!(c.download("seed.txt", &ctx())?, b"sandbox seed\n");
        assert!(c.upload("denied", b"x", &ctx()).is_err());
        command(
            &mut c,
            "if printf bad > seed.txt 2>/dev/null; then exit 1; fi; if touch new 2>/dev/null; then exit 1; fi; echo readonly",
        )?;
        assert_eq!(c.download("seed.txt", &ctx())?, b"sandbox seed\n");
        checks.push("readonly_kernel_and_sdk");
        let record = c.snapshot().clone();
        c.close(&ctx())?;
        gone(&record)?;
    }
    {
        let manager = manager(temp.path(), "tools")?;
        let _cleanup = Cleanup(&manager);
        let mut c = manager.acquire(
            ComputerSpec::new(&image).with_sandbox(Policy::inspect())?,
            &ctx(),
        )?;
        assert!(
            c.shell("touch forbidden", ".", Duration::from_secs(1), &ctx())
                .is_err()
        );
        assert!(c.snapshot().pending.is_none());
        let tools = GuestTools::new(
            c,
            GuestPolicy {
                allow_shell: true,
                allow_write: true,
                ..GuestPolicy::default()
            },
        );
        assert_eq!(tools.definitions().len(), 2);
        checks.push("tools_only_intersects_grants");
        let shared = namzu_computer::SharedGuestTools::new(tools);
        assert!(shared.launcher().check(&ctx()).is_err());
        drop(shared.into_computer()?);
        checks.push("tools_only_mcp_launch_denied");
        assert_eq!(manager.snapshot()?.unwrap().phase, Phase::Closed);
    }
    for kind in ["output", "timeout"] {
        let manager = manager(temp.path(), kind)?;
        let _cleanup = Cleanup(&manager);
        let mut policy = p.clone();
        policy.limits.timeout_seconds = 1;
        let mut c = manager.acquire(ComputerSpec::new(&image).with_sandbox(policy)?, &ctx())?;
        let cmd = if kind == "output" {
            "while :; do printf 0123456789; done"
        } else {
            "sleep 30"
        };
        let result = c.shell(cmd, ".", Duration::from_secs(30), &ctx());
        if kind == "output" {
            let out = result?;
            assert_eq!(out.status, "output_limit");
            assert!(out.stdout.len() + out.stderr.len() <= 3072);
        } else if let Ok(out) = result {
            assert_eq!(out.status, "timeout");
        }
        assert!(matches!(
            c.snapshot().phase,
            Phase::Stopped | Phase::CleanupRequired
        ));
        assert!(c.snapshot().pending.is_some());
        let spec = c.snapshot().spec.clone();
        drop(c);
        assert!(manager.acquire(spec, &ctx()).is_err());
        let record = manager.snapshot()?.unwrap();
        manager.recover_cleanup(&ctx())?;
        gone(&record)?;
        checks.push(if kind == "output" {
            "output_stop_retain_cleanup"
        } else {
            "timeout_stop_retain_no_replay"
        });
    }
    println!(
        "{}",
        serde_json::json!({"checks":checks,"passed":checks.len(),"model_calls":0})
    );
    Ok(())
}
