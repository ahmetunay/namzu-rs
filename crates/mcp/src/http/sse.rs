use super::Error;

/// Incremental SSE framing. Every line, event and stream is bounded by the caller.
pub(super) struct Decoder {
    line: Vec<u8>,
    data: Vec<u8>,
    cr: bool,
    first: bool,
    message: bool,
    limit: usize,
}
impl Decoder {
    pub fn new(limit: usize) -> Self {
        Self {
            line: vec![],
            data: vec![],
            cr: false,
            first: true,
            message: true,
            limit,
        }
    }
    pub fn push(&mut self, byte: u8) -> Result<Option<Vec<u8>>, Error> {
        if byte == b'\n' && self.cr {
            self.cr = false;
            return Ok(None);
        }
        self.cr = byte == b'\r';
        if byte != b'\r' && byte != b'\n' {
            if self.line.len() >= self.limit {
                return Err(Error::Limit);
            }
            self.line.push(byte);
            return Ok(None);
        }
        let mut line = std::mem::take(&mut self.line);
        if self.first {
            self.first = false;
            if line.starts_with(&[239, 187, 191]) {
                line.drain(..3);
            }
        }
        std::str::from_utf8(&line).map_err(|_| Error::Stream)?;
        if line.is_empty() {
            let message = std::mem::replace(&mut self.message, true);
            if !message {
                self.data.clear();
                return Ok(None);
            }
            if self.data.is_empty() {
                return Ok(None);
            }
            self.data.pop(); // last data line's LF
            let data = std::mem::take(&mut self.data);
            return Ok((!data.is_empty()).then_some(data));
        }
        let (field, mut value) = match line.iter().position(|b| *b == b':') {
            Some(i) => (&line[..i], &line[i + 1..]),
            None => (&line[..], &[][..]),
        };
        if value.first() == Some(&b' ') {
            value = &value[1..];
        }
        if field == b"event" {
            self.message = value.is_empty() || value == b"message";
        }
        if field == b"data" {
            if self
                .data
                .len()
                .saturating_add(value.len())
                .saturating_add(1)
                > self.limit + 1
            {
                return Err(Error::Limit);
            }
            self.data.extend_from_slice(value);
            self.data.push(b'\n');
        }
        // id/retry are deliberately not resumed; no reconnect and no POST replay.
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn decode(input: &[u8], limit: usize) -> Result<Vec<Vec<u8>>, Error> {
        let mut decoder = Decoder::new(limit);
        let mut events = vec![];
        for b in input {
            if let Some(data) = decoder.push(*b)? {
                events.push(data);
            }
        }
        Ok(events)
    }
    #[test]
    fn multiline_and_all_sse_line_endings() {
        assert_eq!(decode(b"\xef\xbb\xbf: comment\rdata: first\rdata: second\r\rid: cursor\nretry: 1000\ndata:\n\nevent: message\r\ndata: third\r\n\r\n",100).unwrap(),vec![b"first\nsecond".to_vec(),b"third".to_vec()]);
    }
    #[test]
    fn unknown_events_do_not_become_rpc_messages() {
        assert_eq!(
            decode(b"event: endpoint\ndata: /untrusted\n\ndata: {}\n\n", 100).unwrap(),
            vec![b"{}".to_vec()]
        );
    }
    #[test]
    fn bounds_cover_aggregate_lines_and_utf8() {
        assert_eq!(
            decode(b"data: 12345\ndata: 67890\n\n", 10),
            Err(Error::Limit)
        );
        assert_eq!(decode(b"data: \xff\n\n", 100), Err(Error::Stream));
    }
}
