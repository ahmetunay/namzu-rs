use super::Error;
use std::collections::VecDeque;

pub(super) struct Frames {
    partial: Vec<u8>,
    ready: VecDeque<Vec<u8>>,
    bytes: usize,
    max_frame: usize,
    max_queue: usize,
}
impl Frames {
    pub fn new(max_frame: usize, max_queue: usize) -> Self {
        Self {
            partial: vec![],
            ready: VecDeque::new(),
            bytes: 0,
            max_frame,
            max_queue,
        }
    }
    pub fn push(&mut self, data: &[u8]) -> Result<(), Error> {
        for byte in data {
            if *byte == b'\n' {
                if self.ready.len() >= 64 {
                    return Err(Error::Backpressure);
                }
                let mut line = std::mem::take(&mut self.partial);
                if line.last() == Some(&b'\r') {
                    line.pop();
                    self.bytes -= 1;
                }
                self.ready.push_back(line);
            } else {
                if self.partial.len() >= self.max_frame {
                    return Err(Error::FrameLimit);
                }
                if self.bytes >= self.max_queue {
                    return Err(Error::Backpressure);
                }
                self.partial.push(*byte);
                self.bytes += 1;
            }
        }
        Ok(())
    }
    pub fn pop(&mut self) -> Option<Vec<u8>> {
        let value = self.ready.pop_front()?;
        self.bytes -= value.len();
        Some(value)
    }
    pub fn ready(&self) -> bool {
        !self.ready.is_empty()
    }
    pub fn partial(&self) -> bool {
        !self.partial.is_empty()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragmented_coalesced_crlf_and_multibyte_frames() {
        let mut f = Frames::new(64, 128);
        let bytes = "{\"text\":\"ş😀\"}\r\n{}\n".as_bytes();
        for b in bytes {
            f.push(&[*b]).unwrap();
        }
        assert_eq!(f.pop().unwrap(), "{\"text\":\"ş😀\"}".as_bytes());
        assert_eq!(f.pop().unwrap(), b"{}");
        assert!(f.pop().is_none());
        assert!(!f.partial());
    }
    #[test]
    fn partial_and_complete_queue_limits_are_enforced_before_growth() {
        let mut f = Frames::new(3, 8);
        f.push(b"123").unwrap();
        assert!(matches!(f.push(b"4"), Err(Error::FrameLimit)));
        let mut f = Frames::new(8, 4);
        f.push(b"{}\n{}").unwrap();
        assert!(matches!(f.push(b"x"), Err(Error::Backpressure)));
        let mut f = Frames::new(8, 512);
        for _ in 0..64 {
            f.push(b"{}\n").unwrap();
        }
        assert!(matches!(f.push(b"{}\n"), Err(Error::Backpressure)));
    }
}
