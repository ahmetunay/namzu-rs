use namzu_core::AgentError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
pub const MAX_AUDIO_BYTES: usize = 12 * 1024 * 1024;
pub const MAX_SECONDS: u32 = 60;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AudioInfo {
    pub sample_rate: u32,
    pub channels: u16,
    pub frames: u32,
    pub duration_ms: u64,
    pub size_bytes: usize,
    pub sha256: String,
}
#[derive(Clone)]
pub struct Wav {
    bytes: Vec<u8>,
    info: AudioInfo,
}
impl std::fmt::Debug for Wav {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wav")
            .field("info", &self.info)
            .field("samples", &"<redacted>")
            .finish()
    }
}
fn bad() -> AgentError {
    AgentError::Protocol("invalid or unsupported bounded PCM16 WAV")
}
fn u16le(v: &[u8]) -> u16 {
    u16::from_le_bytes(v.try_into().unwrap())
}
fn u32le(v: &[u8]) -> u32 {
    u32::from_le_bytes(v.try_into().unwrap())
}
fn format_ok(rate: u32, channels: u16) -> bool {
    (8000..=48000).contains(&rate) && (1..=2).contains(&channels)
}
impl Wav {
    pub fn parse(bytes: Vec<u8>) -> Result<Self, AgentError> {
        if bytes.len() < 44
            || bytes.len() > MAX_AUDIO_BYTES
            || &bytes[..4] != b"RIFF"
            || &bytes[8..12] != b"WAVE"
            || u32le(&bytes[4..8]) as usize != bytes.len() - 8
        {
            return Err(bad());
        }
        let mut position = 12usize;
        let mut chunks = 0;
        let mut format = None;
        let mut data = None;
        while position < bytes.len() {
            chunks += 1;
            if chunks > 128 || bytes.len() - position < 8 {
                return Err(bad());
            }
            let kind = &bytes[position..position + 4];
            let len = u32le(&bytes[position + 4..position + 8]) as usize;
            let start = position + 8;
            let end = start.checked_add(len).ok_or_else(bad)?;
            let next = end.checked_add(len % 2).ok_or_else(bad)?;
            if next > bytes.len() {
                return Err(bad());
            }
            match kind {
                b"fmt " => {
                    if format.is_some()
                        || (len != 16 && !(len == 18 && bytes[start + 16..end] == [0, 0]))
                    {
                        return Err(bad());
                    }
                    let f = &bytes[start..end];
                    let channels = u16le(&f[2..4]);
                    let rate = u32le(&f[4..8]);
                    if u16le(&f[..2]) != 1
                        || !format_ok(rate, channels)
                        || u16le(&f[14..16]) != 16
                        || u16le(&f[12..14]) != channels * 2
                        || u32le(&f[8..12]) != rate * u32::from(channels) * 2
                    {
                        return Err(bad());
                    }
                    format = Some((rate, channels));
                }
                b"data" if data.is_some() => return Err(bad()),
                b"data" => data = Some(len),
                _ => (),
            }
            position = next;
        }
        let (sample_rate, channels) = format.ok_or_else(bad)?;
        let data = data.ok_or_else(bad)?;
        let align = usize::from(channels) * 2;
        if data == 0
            || data % align != 0
            || data / align > sample_rate as usize * MAX_SECONDS as usize
        {
            return Err(bad());
        }
        let frames = (data / align) as u32;
        let info = AudioInfo {
            sample_rate,
            channels,
            frames,
            duration_ms: u64::from(frames) * 1000 / u64::from(sample_rate),
            size_bytes: bytes.len(),
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        };
        Ok(Self { bytes, info })
    }
    /// Normalize unknown RIFF/data lengths only after a complete bounded HTTP body.
    /// Local file parsing stays strict; PCM bytes are never resampled or truncated.
    pub fn from_http_body(mut bytes: Vec<u8>) -> Result<Self, AgentError> {
        if bytes.len() < 44
            || bytes.len() > MAX_AUDIO_BYTES
            || &bytes[..4] != b"RIFF"
            || &bytes[8..12] != b"WAVE"
        {
            return Err(bad());
        }
        if u32le(&bytes[4..8]) != u32::MAX {
            return Self::parse(bytes);
        }
        let size = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&size.to_le_bytes());
        let mut position = 12usize;
        let mut chunks = 0;
        while position < bytes.len() {
            chunks += 1;
            if chunks > 128 || bytes.len() - position < 8 {
                return Err(bad());
            }
            let len = u32le(&bytes[position + 4..position + 8]);
            let start = position + 8;
            if &bytes[position..position + 4] == b"data" && len == u32::MAX {
                let actual = (bytes.len() - start) as u32;
                bytes[position + 4..position + 8].copy_from_slice(&actual.to_le_bytes());
                break;
            }
            position = start
                .checked_add(len as usize)
                .and_then(|v| v.checked_add((len % 2) as usize))
                .filter(|v| *v <= bytes.len())
                .ok_or_else(bad)?;
        }
        Self::parse(bytes)
    }
    pub fn pcm16(sample_rate: u32, channels: u16, samples: &[i16]) -> Result<Self, AgentError> {
        if !format_ok(sample_rate, channels)
            || samples.is_empty()
            || !samples.len().is_multiple_of(usize::from(channels))
            || samples.len() > sample_rate as usize * usize::from(channels) * MAX_SECONDS as usize
        {
            return Err(bad());
        }
        let size = samples.len() * 2;
        if size + 44 > MAX_AUDIO_BYTES {
            return Err(bad());
        }
        let mut bytes = Vec::with_capacity(size + 44);
        bytes.extend(b"RIFF");
        bytes.extend(((size + 36) as u32).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(channels.to_le_bytes());
        bytes.extend(sample_rate.to_le_bytes());
        bytes.extend((sample_rate * u32::from(channels) * 2).to_le_bytes());
        bytes.extend((channels * 2).to_le_bytes());
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend((size as u32).to_le_bytes());
        for sample in samples {
            bytes.extend(sample.to_le_bytes());
        }
        Self::parse(bytes)
    }
    pub fn info(&self) -> &AudioInfo {
        &self.info
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}
