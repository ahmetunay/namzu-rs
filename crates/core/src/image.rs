//! Bounded PNG tool data, independent of the computer/provider packages.
use crate::AgentError;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
pub const IMAGE_BYTES: usize = 1024 * 1024;
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ToolImage {
    pub width: u16,
    pub height: u16,
    pub png_base64: String,
}
impl std::fmt::Debug for ToolImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("png_base64", &"<redacted>")
            .finish()
    }
}
impl ToolImage {
    pub fn png(&self) -> Result<Vec<u8>, AgentError> {
        if self.width == 0
            || self.width > 1920
            || self.height == 0
            || self.height > 1080
            || self.png_base64.len() > IMAGE_BYTES.div_ceil(3) * 4
        {
            return Err(AgentError::Protocol("tool image bounds"));
        }
        let data = STANDARD
            .decode(&self.png_base64)
            .map_err(|_| AgentError::Protocol("invalid tool image base64"))?;
        if data.len() > IMAGE_BYTES {
            return Err(AgentError::Protocol("tool image byte limit"));
        }
        let mut decoder = png::Decoder::new(data.as_slice());
        decoder.set_limits(png::Limits {
            bytes: 16 * 1024 * 1024,
        });
        let mut reader = decoder
            .read_info()
            .map_err(|_| AgentError::Protocol("invalid tool PNG"))?;
        let info = reader.info();
        if info.width != u32::from(self.width)
            || info.height != u32::from(self.height)
            || info.color_type != png::ColorType::Rgb
            || info.bit_depth != png::BitDepth::Eight
            || info.animation_control.is_some()
        {
            return Err(AgentError::Protocol("unexpected tool PNG format"));
        }
        let mut pixels = vec![0; reader.output_buffer_size()];
        reader
            .next_frame(&mut pixels)
            .map_err(|_| AgentError::Protocol("invalid tool PNG pixels"))?;
        reader
            .finish()
            .map_err(|_| AgentError::Protocol("invalid tool PNG trailer"))?;
        Ok(data)
    }
    pub fn data_url(&self) -> String {
        format!("data:image/png;base64,{}", self.png_base64)
    }
}
