//! Unix process-group supervision. This is host execution, not OS confinement.
use namzu_core::{AgentError, TurnContext};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub struct ProcessSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub timeout: Duration,
    pub output_limit: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProcessOutput {
    pub status: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub peak_rss_kib: Option<u64>,
}

struct OwnedChild {
    child: Child,
    reaped: bool,
    group_killed: bool,
    peak_rss_kib: Option<u64>,
}
impl OwnedChild {
    fn new(child: Child) -> Self {
        Self {
            child,
            reaped: false,
            group_killed: false,
            peak_rss_kib: None,
        }
    }
    fn kill_group(&mut self) {
        if !self.group_killed {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGKILL);
            }
            if !self.reaped {
                let _ = self.child.kill();
            }
            self.group_killed = true;
        }
    }
    #[cfg(unix)]
    fn wait(&mut self, blocking: bool) -> io::Result<Option<std::process::ExitStatus>> {
        use std::os::unix::process::ExitStatusExt;
        if self.reaped {
            return Ok(None);
        }
        loop {
            let mut status = 0;
            let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
            let result = unsafe {
                libc::wait4(
                    self.child.id() as i32,
                    &mut status,
                    if blocking { 0 } else { libc::WNOHANG },
                    &mut usage,
                )
            };
            if result < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if result == 0 {
                return Ok(None);
            }
            self.reaped = true;
            let rss = usage.ru_maxrss.max(0) as u64;
            #[cfg(target_os = "macos")]
            let rss = rss / 1024;
            self.peak_rss_kib = Some(rss);
            return Ok(Some(std::process::ExitStatus::from_raw(status)));
        }
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.kill_group();
        #[cfg(unix)]
        let _ = self.wait(true);
    }
}

#[cfg(unix)]
fn nonblocking(pipe: &impl std::os::fd::AsRawFd) -> io::Result<()> {
    let fd = pipe.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
fn drain(pipe: &mut impl Read, bytes: &mut Vec<u8>, left: &mut usize) -> io::Result<bool> {
    for _ in 0..16 {
        let mut block = [0u8; 4096];
        match pipe.read(&mut block) {
            Ok(0) => return Ok(false),
            Ok(n) => {
                let keep = n.min(*left);
                bytes.extend_from_slice(&block[..keep]);
                *left -= keep;
                if keep < n {
                    return Ok(true);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(false)
}

pub fn run_process(spec: ProcessSpec, context: &TurnContext) -> Result<ProcessOutput, AgentError> {
    run_process_input(spec, context, &[])
}

/// Concurrent bounded stdin/stdout: a guest that writes before reading cannot deadlock us.
pub fn run_process_input(
    spec: ProcessSpec,
    context: &TurnContext,
    input: &[u8],
) -> Result<ProcessOutput, AgentError> {
    #[cfg(not(unix))]
    {
        let _ = (spec, context, input);
        return Err(AgentError::Configuration(
            "process supervision currently requires Unix",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        context.remaining()?;
        if spec.output_limit == 0
            || spec.output_limit > 16 * 1024 * 1024
            || spec.timeout.is_zero()
            || input.len() > 4 * 1024 * 1024
        {
            return Err(AgentError::Configuration("invalid process limits"));
        }
        let timeout = spec.timeout.min(context.remaining()?);
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or(AgentError::Configuration("process timeout overflow"))?;
        let mut command = Command::new(spec.program);
        command
            .args(spec.args)
            .current_dir(spec.cwd)
            .env_clear()
            .envs(spec.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let mut child = OwnedChild::new(
            command
                .spawn()
                .map_err(|_| AgentError::Provider("cannot launch process".into()))?,
        );
        let mut out = child
            .child
            .stdout
            .take()
            .ok_or(AgentError::Protocol("missing process stdout"))?;
        let mut stdin = child.child.stdin.take();
        if let Some(pipe) = &stdin {
            nonblocking(pipe)
                .map_err(|_| AgentError::Provider("cannot configure process stdin".into()))?;
        }
        let mut written = 0;
        let mut err = child
            .child
            .stderr
            .take()
            .ok_or(AgentError::Protocol("missing process stderr"))?;
        nonblocking(&out)
            .and_then(|_| nonblocking(&err))
            .map_err(|_| AgentError::Provider("cannot configure process pipes".into()))?;
        let mut stdout = vec![];
        let mut stderr = vec![];
        let mut left = spec.output_limit;
        loop {
            context.remaining()?;
            if let Some(pipe) = &mut stdin {
                match pipe.write(&input[written..]) {
                    Ok(n) => written += n,
                    Err(e)
                        if e.kind() == io::ErrorKind::WouldBlock
                            || e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {
                        written = input.len();
                    }
                    Err(_) => {
                        return Err(AgentError::Provider("cannot write process stdin".into()));
                    }
                }
                if written == input.len() {
                    stdin.take();
                }
            }
            let exceeded = drain(&mut out, &mut stdout, &mut left)
                .and_then(|a| drain(&mut err, &mut stderr, &mut left).map(|b| a || b))
                .map_err(|_| AgentError::Provider("cannot read process output".into()))?;
            let outcome = if exceeded {
                Some(("output_limit", None))
            } else if Instant::now() >= deadline {
                Some(("timeout", None))
            } else {
                child
                    .wait(false)
                    .map_err(|_| AgentError::Provider("cannot wait for process".into()))?
                    .map(|status| ("exited", status.code()))
            };
            if let Some((status, exit_code)) = outcome {
                // Kill inherited-pipe/background descendants even when leader exited.
                child.kill_group();
                let _ = child.wait(true);
                let more = drain(&mut out, &mut stdout, &mut left)
                    .and_then(|a| drain(&mut err, &mut stderr, &mut left).map(|b| a || b))
                    .unwrap_or(false);
                return Ok(ProcessOutput {
                    status: if more && status == "exited" {
                        "output_limit".into()
                    } else {
                        status.into()
                    },
                    exit_code,
                    peak_rss_kib: child.peak_rss_kib,
                    stdout: String::from_utf8_lossy(&stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&stderr).into_owned(),
                });
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
