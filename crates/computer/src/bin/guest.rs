//! Native guest bridge. Never invoked as a host fallback.
use base64::{Engine, engine::general_purpose::STANDARD};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions, OpenOptionsExt},
};
use namzu_computer::protocol::{
    FILE_LIMIT, GuestReply, GuestRequest, GuestValue, WIRE_LIMIT, valid_path,
};
use namzu_core::{Cancellation, TurnContext};
use namzu_tools::process::{ProcessSpec, run_process};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    time::Duration,
};

fn files(request: GuestRequest) -> Result<GuestValue, String> {
    let root = Dir::open_ambient_dir("/workspace", ambient_authority())
        .map_err(|_| "guest workspace unavailable")?;
    match request {
        GuestRequest::Desktop {
            request,
            timeout_ms,
        } => {
            if !(1..=15000).contains(&timeout_ms) {
                return Err("invalid desktop deadline".into());
            }
            let input = serde_json::to_vec(&request).map_err(|_| "invalid desktop input")?;
            let context =
                TurnContext::new(Duration::from_millis(timeout_ms), Cancellation::default())
                    .map_err(|_| "invalid desktop deadline")?;
            let out = namzu_tools::process::run_process_input(
                ProcessSpec {
                    program: "/usr/local/bin/namzu-desktop".into(),
                    args: vec![],
                    cwd: "/workspace".into(),
                    env: BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]),
                    timeout: Duration::from_millis(timeout_ms),
                    output_limit: namzu_computer::desktop::SCREEN_WIRE,
                },
                &context,
                &input,
            )
            .map_err(|_| "desktop deadline; computer must stop")?;
            if out.status != "exited" || out.exit_code != Some(0) {
                return Err("desktop worker failed".into());
            }
            let value: Result<namzu_computer::desktop::Reply, String> =
                serde_json::from_str(&out.stdout).map_err(|_| "invalid desktop reply")?;
            value.map(|value| GuestValue::Desktop { value })
        }
        GuestRequest::Probe => Ok(GuestValue::Ready),
        GuestRequest::Read { path } => {
            if !valid_path(&path) {
                return Err("invalid guest path".into());
            }
            let mut options = OpenOptions::new();
            options.read(true).custom_flags(libc::O_NONBLOCK);
            let file = root
                .open_with(&path, &options)
                .map_err(|_| "cannot read guest file")?;
            if !file
                .metadata()
                .map_err(|_| "cannot inspect guest file")?
                .is_file()
            {
                return Err("regular guest file required".into());
            }
            let mut data = vec![];
            file.take((FILE_LIMIT + 1) as u64)
                .read_to_end(&mut data)
                .map_err(|_| "cannot read guest file")?;
            if data.len() > FILE_LIMIT {
                return Err("guest file limit".into());
            }
            Ok(GuestValue::Bytes {
                base64: STANDARD.encode(data),
            })
        }
        GuestRequest::Write { path, base64 } => {
            if !valid_path(&path) || base64.len() > FILE_LIMIT.div_ceil(3) * 4 {
                return Err("invalid guest write".into());
            }
            let bytes = STANDARD
                .decode(base64)
                .map_err(|_| "invalid file encoding")?;
            if bytes.len() > FILE_LIMIT {
                return Err("guest file limit".into());
            }
            let path = std::path::Path::new(&path);
            let leaf = path.file_name().ok_or("file name required")?;
            let parent = root
                .open_dir(
                    path.parent()
                        .filter(|p| !p.as_os_str().is_empty())
                        .unwrap_or(std::path::Path::new(".")),
                )
                .map_err(|_| "cannot open guest parent")?;
            let temporary = format!(".namzu-upload-{}", std::process::id());
            let mut opts = OpenOptions::new();
            opts.write(true).create_new(true).mode(0o600);
            let result = (|| {
                let mut file = parent
                    .open_with(&temporary, &opts)
                    .map_err(|_| "cannot create guest temporary")?;
                file.write_all(&bytes)
                    .and_then(|_| file.sync_all())
                    .map_err(|_| "cannot sync guest write")?;
                parent
                    .rename(&temporary, &parent, leaf)
                    .map_err(|_| "cannot publish guest file")?;
                parent
                    .open(".")
                    .and_then(|f| f.sync_all())
                    .map_err(|_| "cannot sync guest directory")?;
                Ok(GuestValue::Written)
            })();
            if result.is_err() {
                let _ = parent.remove_file(&temporary);
            }
            result
        }
        GuestRequest::List { path } => {
            if !valid_path(&path) {
                return Err("invalid guest path".into());
            }
            let mut names = vec![];
            for entry in root
                .read_dir(&path)
                .map_err(|_| "cannot list guest directory")?
            {
                if names.len() >= 512 {
                    return Err("guest directory limit".into());
                }
                names.push(
                    entry
                        .map_err(|_| "cannot list guest entry")?
                        .file_name()
                        .to_str()
                        .ok_or("non UTF-8 guest name")?
                        .to_owned(),
                );
            }
            names.sort();
            Ok(GuestValue::Entries { names })
        }
        GuestRequest::Shell {
            command,
            cwd,
            timeout_ms,
        } => {
            if command.is_empty()
                || command.len() > FILE_LIMIT
                || !valid_path(&cwd)
                || !(1..=300_000).contains(&timeout_ms)
            {
                return Err("invalid guest shell request".into());
            }
            let cwd = std::path::Path::new("/workspace")
                .join(cwd)
                .canonicalize()
                .map_err(|_| "invalid guest shell cwd")?;
            if !cwd.starts_with("/workspace") || !cwd.is_dir() {
                return Err("guest cwd escapes workspace".into());
            }
            // Adopt double-fork/setsid descendants so normal completion also cleans them.
            if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } != 0 {
                return Err("guest subreaper unavailable".into());
            }
            let context =
                TurnContext::new(Duration::from_millis(timeout_ms), Cancellation::default())
                    .map_err(|_| "invalid guest timeout")?;
            let mut env = BTreeMap::from([
                ("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into()),
                ("HOME".into(), "/workspace".into()),
                ("LANG".into(), "C.UTF-8".into()),
            ]);
            for key in [
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "NO_PROXY",
                "http_proxy",
                "https_proxy",
                "no_proxy",
            ] {
                if let Ok(value) = std::env::var(key) {
                    env.insert(key.into(), value);
                }
            }
            let output = run_process(
                ProcessSpec {
                    program: "/bin/sh".into(),
                    args: vec!["-c".into(), command],
                    cwd,
                    env,
                    timeout: Duration::from_millis(timeout_ms),
                    output_limit: FILE_LIMIT,
                },
                &context,
            );
            reap_descendants()?;
            output
                .map(|output| GuestValue::Shell { output })
                .map_err(|_| "guest shell deadline/cancellation failed; computer must stop".into())
        }
    }
}
fn reap_descendants() -> Result<(), String> {
    for _ in 0..256 {
        // Some managed kernels omit /proc/.../children; PPid in stat is portable on Linux.
        for entry in std::fs::read_dir("/proc").map_err(|_| "cannot inspect guest descendants")? {
            let entry = entry.map_err(|_| "cannot inspect guest PID")?;
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<i32>().ok())
            else {
                continue;
            };
            let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
                continue;
            };
            let Some((_, fields)) = stat.rsplit_once(')') else {
                continue;
            };
            let parent = fields
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse::<u32>().ok());
            if parent != Some(std::process::id()) {
                continue;
            }
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
        loop {
            let mut status = 0;
            let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
            if pid > 0 {
                continue;
            }
            if pid < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD) {
                return Ok(());
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Err("guest descendant cleanup failed; computer must stop".into())
}
fn main() {
    if std::env::args().nth(1).as_deref() == Some("idle") {
        if let Ok(screen) = std::env::var("NAMZU_DESKTOP_SCREEN") {
            let mut server = std::process::Command::new("/usr/bin/Xvfb")
                .args([
                    ":99",
                    "-screen",
                    "0",
                    &screen,
                    "-nolisten",
                    "tcp",
                    "-ac",
                    "-noreset",
                ])
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("desktop server unavailable");
            loop {
                if server.try_wait().ok().flatten().is_some() {
                    std::process::exit(1)
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        loop {
            std::thread::park();
        }
    }
    let result = (|| {
        let mut bytes = vec![];
        std::io::stdin()
            .take((WIRE_LIMIT + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| "cannot read guest request")?;
        if bytes.len() > WIRE_LIMIT {
            return Err("guest request limit".into());
        }
        files(serde_json::from_slice(&bytes).map_err(|_| "invalid guest protocol")?)
    })();
    let reply = GuestReply { version: 1, result };
    match serde_json::to_vec(&reply) {
        Ok(bytes) if bytes.len() <= namzu_computer::desktop::SCREEN_WIRE => {
            let _ = std::io::stdout().write_all(&bytes);
        }
        _ => std::process::exit(1),
    }
}
