use super::{Error, Spec};
use std::{
    io,
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

pub(super) struct Process {
    pub child: Child,
    stopped: bool,
    reaped: bool,
    pub status: Option<ExitStatus>,
}
impl Process {
    pub fn spawn(spec: &Spec) -> Result<Self, Error> {
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .current_dir(&spec.cwd)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin");
        for key in [
            "LANG",
            "LC_ALL",
            "TZ",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
            "no_proxy",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
            "REQUESTS_CA_BUNDLE",
            "CURL_CA_BUNDLE",
            "NODE_EXTRA_CA_CERTS",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command
            .envs(&spec.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        #[cfg(target_os = "linux")]
        {
            let parent = unsafe { libc::getpid() };
            unsafe {
                command.pre_exec(move || {
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                        return Err(io::Error::last_os_error());
                    }
                    if libc::getppid() != parent {
                        return Err(io::Error::other("parent exited"));
                    }
                    Ok(())
                });
            }
        }
        let child = command.spawn().map_err(|_| Error::Spawn)?;
        Ok(Self {
            child,
            stopped: false,
            reaped: false,
            status: None,
        })
    }
    pub fn exited(&self) -> Result<bool, Error> {
        if self.reaped {
            return Ok(true);
        }
        // Do not reap before killing descendants: the unreaped leader reserves
        // its PID/PGID so a later group signal cannot hit a reused identity.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                self.child.id(),
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                return Ok(false);
            }
            return Err(Error::Io);
        }
        Ok(unsafe { info.si_pid() } != 0)
    }
    fn signal(&mut self, signal: i32) {
        if !self.stopped {
            unsafe {
                libc::kill(-(self.child.id() as i32), signal);
            }
        }
    }
    pub fn stop(&mut self, grace: Duration) -> bool {
        if self.stopped {
            return false;
        }
        let mut forced = false;
        // Caller has closed stdin first. Always kill inherited-pipe children,
        // even after an otherwise graceful leader exit.
        if !self.exited().unwrap_or(false) {
            let deadline = Instant::now() + grace;
            while !self.exited().unwrap_or(false) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(2));
            }
            if !self.exited().unwrap_or(false) {
                forced = true;
                self.signal(libc::SIGTERM);
                let deadline = Instant::now() + grace;
                while !self.exited().unwrap_or(false) && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
        self.signal(libc::SIGKILL);
        self.stopped = true;
        if let Ok(status) = self.child.wait() {
            self.status = Some(status);
            self.reaped = true;
        }
        forced
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.stop(Duration::ZERO);
    }
}
pub(super) fn nonblocking(pipe: &impl AsRawFd) -> Result<(), Error> {
    let fd = pipe.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(Error::Io);
    }
    Ok(())
}
