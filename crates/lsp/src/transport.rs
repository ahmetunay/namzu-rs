use crate::{
    Spec,
    process::{Process, nonblocking},
    workspace::bad,
};
use namzu_core::{AgentError, TurnContext};
use serde_json::Value;
use std::{
    io::{Read, Write},
    process::{ChildStderr, ChildStdin, ChildStdout},
    time::Duration,
};
pub const FRAME_BYTES: usize = 1024 * 1024;
#[derive(Default)]
pub(crate) struct Frames {
    buffer: Vec<u8>,
    length: Option<usize>,
}
impl Frames {
    pub fn push(&mut self, bytes: &[u8]) -> Result<(), AgentError> {
        if self.buffer.len() + bytes.len() > FRAME_BYTES + 16384 {
            return Err(bad("LSP receive buffer limit"));
        }
        self.buffer.extend_from_slice(bytes);
        Ok(())
    }
    pub fn next(&mut self) -> Result<Option<Value>, AgentError> {
        if self.length.is_none() {
            let Some(at) = self.buffer.windows(4).position(|v| v == b"\r\n\r\n") else {
                if self.buffer.len() > 8192 {
                    return Err(bad("LSP header limit"));
                }
                return Ok(None);
            };
            if at > 8192 {
                return Err(bad("LSP header limit"));
            }
            let header =
                std::str::from_utf8(&self.buffer[..at]).map_err(|_| bad("invalid LSP header"))?;
            let mut length = None;
            let mut content_type = false;
            for line in header.split("\r\n") {
                let (name, value) = line
                    .split_once(':')
                    .ok_or(bad("invalid LSP header field"))?;
                match name.to_ascii_lowercase().as_str() {
                    "content-length" => {
                        if length.is_some()
                            || value.trim().is_empty()
                            || !value.trim().bytes().all(|b| b.is_ascii_digit())
                        {
                            return Err(bad("invalid LSP Content-Length"));
                        }
                        length = Some(
                            value
                                .trim()
                                .parse::<usize>()
                                .map_err(|_| bad("invalid LSP Content-Length"))?,
                        );
                    }
                    "content-type" if !content_type && value.len() <= 256 => content_type = true,
                    _ => return Err(bad("unsupported or duplicate LSP header")),
                }
            }
            let length = length.ok_or(bad("LSP Content-Length missing"))?;
            if length == 0 || length > FRAME_BYTES {
                return Err(bad("LSP frame limit"));
            }
            self.buffer.drain(..at + 4);
            self.length = Some(length);
        }
        let length = self.length.unwrap();
        if self.buffer.len() < length {
            return Ok(None);
        }
        let value =
            serde_json::from_slice(&self.buffer[..length]).map_err(|_| bad("invalid LSP JSON"))?;
        self.buffer.drain(..length);
        self.length = None;
        Ok(Some(value))
    }
}
#[derive(Debug, serde::Serialize)]
pub struct CloseReport {
    pub exit_code: Option<i32>,
    pub forced: bool,
    pub stderr_bytes: usize,
}
pub(crate) struct Transport {
    process: Process,
    input: Option<ChildStdin>,
    output: ChildStdout,
    error: ChildStderr,
    frames: Frames,
    stdout_bytes: usize,
    stderr_bytes: usize,
    closed: bool,
    forced: bool,
}
impl Transport {
    pub fn spawn(spec: &Spec) -> Result<Self, AgentError> {
        let mut process = Process::spawn(spec)?;
        let input = process.child.stdin.take().ok_or(bad("LSP stdin missing"))?;
        let output = process
            .child
            .stdout
            .take()
            .ok_or(bad("LSP stdout missing"))?;
        let error = process
            .child
            .stderr
            .take()
            .ok_or(bad("LSP stderr missing"))?;
        nonblocking(&input)?;
        nonblocking(&output)?;
        nonblocking(&error)?;
        Ok(Self {
            process,
            input: Some(input),
            output,
            error,
            frames: Frames::default(),
            stdout_bytes: 0,
            stderr_bytes: 0,
            closed: false,
            forced: false,
        })
    }
    fn pump(&mut self, ctx: &TurnContext) -> Result<(), AgentError> {
        ctx.remaining()?;
        if self.closed {
            return Err(bad("LSP transport closed"));
        }
        let mut block = [0; 8192];
        for _ in 0..8 {
            match self.error.read(&mut block) {
                Ok(0) => break,
                Ok(n) => {
                    self.stderr_bytes += n;
                    if self.stderr_bytes > 1024 * 1024 {
                        return Err(bad("LSP stderr budget exceeded"));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(bad("LSP stderr read failed")),
            }
        }
        match self.output.read(&mut block) {
            Ok(0) => return Err(bad("LSP EOF or truncated frame")),
            Ok(n) => {
                self.stdout_bytes += n;
                if self.stdout_bytes > 16 * 1024 * 1024 {
                    return Err(bad("LSP stdout budget exceeded"));
                }
                self.frames.push(&block[..n])?;
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return Err(bad("LSP stdout read failed")),
        }
        Ok(())
    }
    fn wait(ctx: &TurnContext) -> Result<(), AgentError> {
        let ms = ctx.remaining()?.as_millis().min(2) as i32;
        unsafe {
            libc::poll(std::ptr::null_mut(), 0, ms.max(1));
        }
        ctx.remaining().map(|_| ())
    }
    pub fn send(&mut self, value: &Value, ctx: &TurnContext) -> Result<(), AgentError> {
        ctx.remaining()?;
        if self.closed {
            return Err(bad("LSP transport closed"));
        }
        let body = serde_json::to_vec(value).map_err(|_| bad("cannot encode LSP request"))?;
        if body.len() > FRAME_BYTES {
            return Err(bad("LSP outgoing frame limit"));
        }
        let mut bytes = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        bytes.extend(body);
        let mut offset = 0;
        while offset < bytes.len() {
            ctx.remaining()?;
            match self
                .input
                .as_mut()
                .ok_or(bad("LSP stdin closed"))?
                .write(&bytes[offset..])
            {
                Ok(0) => return Err(bad("LSP stdin EOF")),
                Ok(n) => offset += n,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return Err(bad("LSP stdin write failed")),
            }
            if offset < bytes.len() {
                self.pump(ctx)?;
                Self::wait(ctx)?;
            }
        }
        Ok(())
    }
    pub fn receive(&mut self, ctx: &TurnContext) -> Result<Value, AgentError> {
        loop {
            ctx.remaining()?;
            if let Some(value) = self.frames.next()? {
                return Ok(value);
            }
            self.pump(ctx)?;
            Self::wait(ctx)?;
        }
    }
    pub fn stop(&mut self, grace: Duration) -> CloseReport {
        self.input.take();
        self.forced |= self.process.stop(grace);
        let forced = self.forced;
        self.closed = true;
        CloseReport {
            exit_code: self.process.status.and_then(|s| s.code()),
            forced,
            stderr_bytes: self.stderr_bytes,
        }
    }
}
impl Drop for Transport {
    fn drop(&mut self) {
        if !self.closed {
            self.stop(Duration::ZERO);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn byte_fragments_and_coalesced_utf8_frames() {
        let value = json!({"message":"Türkçe 🌍"});
        let body = serde_json::to_vec(&value).unwrap();
        let mut wire = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        wire.extend(body);
        let mut f = Frames::default();
        for byte in &wire[..wire.len() - 1] {
            f.push(&[*byte]).unwrap();
            assert!(f.next().unwrap().is_none());
        }
        f.push(&wire[wire.len() - 1..]).unwrap();
        assert_eq!(f.next().unwrap(), Some(value.clone()));
        let mut f = Frames::default();
        f.push(&[wire.clone(), wire].concat()).unwrap();
        assert_eq!(f.next().unwrap(), Some(value.clone()));
        assert_eq!(f.next().unwrap(), Some(value));
        assert!(f.next().unwrap().is_none());
    }
    #[test]
    fn ambiguous_missing_and_oversized_lengths_rejected_before_body() {
        for h in [
            "Content-Length: 2\r\ncontent-length: 2\r\n\r\n",
            "Content-Type: application/json\r\n\r\n",
            "Content-Length: -1\r\n\r\n",
            "Content-Length: 1048577\r\n\r\n",
            "Content-Length: 184467440737095516160\r\n\r\n",
        ] {
            let mut f = Frames::default();
            f.push(h.as_bytes()).unwrap();
            assert!(f.next().is_err());
        }
    }
    #[test]
    fn header_and_receive_queue_bounds_hold_without_delimiter() {
        let mut f = Frames::default();
        f.push(&vec![b'x'; 8193]).unwrap();
        assert!(f.next().is_err());
        let mut f = Frames::default();
        assert!(f.push(&vec![0; FRAME_BYTES + 16385]).is_err());
    }
    #[test]
    fn malformed_json_is_rejected_after_exact_declared_body() {
        let mut f = Frames::default();
        f.push(b"Content-Length: 1\r\n\r\n{").unwrap();
        assert!(f.next().is_err());
    }
}
