//! Unix process-group supervision. This is host execution, not OS confinement.
use namzu_core::{AgentError, TurnContext};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    io::{self, Read},
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

#[derive(Debug, Serialize)]
pub struct ProcessOutput {
    pub status: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

struct OwnedChild(Child);
impl OwnedChild {
    fn kill_group(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.kill();
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.kill_group();
        let _ = self.0.wait();
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
    #[cfg(not(unix))]
    {
        let _ = (spec, context);
        return Err(AgentError::Configuration(
            "process supervision currently requires Unix",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        context.remaining()?;
        if spec.output_limit == 0 || spec.output_limit > 4 * 1024 * 1024 || spec.timeout.is_zero() {
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
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let mut child = OwnedChild(
            command
                .spawn()
                .map_err(|_| AgentError::Provider("cannot launch process".into()))?,
        );
        let mut out = child
            .0
            .stdout
            .take()
            .ok_or(AgentError::Protocol("missing process stdout"))?;
        let mut err = child
            .0
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
            let exceeded = drain(&mut out, &mut stdout, &mut left)
                .and_then(|a| drain(&mut err, &mut stderr, &mut left).map(|b| a || b))
                .map_err(|_| AgentError::Provider("cannot read process output".into()))?;
            let outcome = if exceeded {
                Some(("output_limit", None))
            } else if Instant::now() >= deadline {
                Some(("timeout", None))
            } else {
                child
                    .0
                    .try_wait()
                    .map_err(|_| AgentError::Provider("cannot wait for process".into()))?
                    .map(|status| ("exited", status.code()))
            };
            if let Some((status, exit_code)) = outcome {
                // Kill inherited-pipe/background descendants even when leader exited.
                child.kill_group();
                let _ = child.0.wait();
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
                    stdout: String::from_utf8_lossy(&stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&stderr).into_owned(),
                });
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
