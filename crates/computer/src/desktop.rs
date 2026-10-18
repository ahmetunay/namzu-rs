//! Owned guest screen/input SDK; no host display fallback or model tools.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use namzu_core::{AgentError, TurnContext};
use serde::{Deserialize, Serialize};
pub const SCREEN_BYTES: usize = 8 * 1024 * 1024;
pub const SCREEN_WIRE: usize = 12 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScreenConfig {
    pub width: u16,
    pub height: u16,
}
impl Default for ScreenConfig {
    fn default() -> Self {
        Self {
            width: 1024,
            height: 768,
        }
    }
}
impl ScreenConfig {
    pub fn validate(&self) -> Result<(), AgentError> {
        if !(320..=1920).contains(&self.width) || !(240..=1080).contains(&self.height) {
            return Err(AgentError::Configuration(
                "screen dimensions outside 320..1920 x 240..1080",
            ));
        }
        Ok(())
    }
    fn point(&self, x: i16, y: i16) -> bool {
        x >= 0 && y >= 0 && x < self.width as i16 && y < self.height as i16
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Left,
    Middle,
    Right,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Info,
    Screenshot,
    Move {
        x: i16,
        y: i16,
    },
    Click {
        x: i16,
        y: i16,
        button: Button,
    },
    Drag {
        from_x: i16,
        from_y: i16,
        to_x: i16,
        to_y: i16,
    },
    Scroll {
        horizontal: i8,
        vertical: i8,
    },
    Key {
        keys: Vec<String>,
    },
    Type {
        text: String,
    },
}
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DesktopRequest(<redacted>)")
    }
}
impl Request {
    pub fn validate(&self, s: &ScreenConfig) -> Result<(), AgentError> {
        s.validate()?;
        let valid = match self {
            Self::Info | Self::Screenshot => true,
            Self::Move { x, y } | Self::Click { x, y, .. } => s.point(*x, *y),
            Self::Drag {
                from_x,
                from_y,
                to_x,
                to_y,
            } => s.point(*from_x, *from_y) && s.point(*to_x, *to_y),
            Self::Scroll {
                horizontal,
                vertical,
            } => {
                horizontal.abs_diff(0) <= 20
                    && vertical.abs_diff(0) <= 20
                    && (*horizontal != 0 || *vertical != 0)
            }
            Self::Type { text } => {
                !text.is_empty()
                    && text.len() <= 1024
                    && text
                        .bytes()
                        .all(|b| b == b'\n' || b == b'\t' || (32..=126).contains(&b))
            }
            Self::Key { keys } => {
                !keys.is_empty()
                    && keys.len() <= 5
                    && keys.iter().enumerate().all(|(i, k)| {
                        if i + 1 == keys.len() {
                            key_sym(k).is_some() && !modifier(k)
                        } else {
                            modifier(k)
                        }
                    })
                    && keys.iter().collect::<std::collections::BTreeSet<_>>().len() == keys.len()
            }
        };
        if valid {
            Ok(())
        } else {
            Err(AgentError::Configuration(
                "invalid or unsupported guest input",
            ))
        }
    }
    pub fn readonly(&self) -> bool {
        matches!(self, Self::Info | Self::Screenshot)
    }
}
pub fn modifier(s: &str) -> bool {
    matches!(s, "CTRL" | "SHIFT" | "ALT" | "SUPER")
}
pub fn key_sym(s: &str) -> Option<u32> {
    Some(match s {
        "CTRL" => 0xffe3,
        "SHIFT" => 0xffe1,
        "ALT" => 0xffe9,
        "SUPER" => 0xffeb,
        "ENTER" => 0xff0d,
        "TAB" => 0xff09,
        "ESC" => 0xff1b,
        "BACKSPACE" => 0xff08,
        "DELETE" => 0xffff,
        "LEFT" => 0xff51,
        "UP" => 0xff52,
        "RIGHT" => 0xff53,
        "DOWN" => 0xff54,
        "HOME" => 0xff50,
        "END" => 0xff57,
        "PAGEUP" => 0xff55,
        "PAGEDOWN" => 0xff56,
        _ => {
            if s.len() == 1 && s.as_bytes()[0].is_ascii_graphic() || s == " " {
                s.as_bytes()[0] as u32
            } else {
                let n = s
                    .strip_prefix('F')
                    .and_then(|n| n.parse::<u32>().ok())
                    .filter(|n| (1..=12).contains(n))?;
                0xffbd + n
            }
        }
    })
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Reply {
    Info {
        width: u16,
        height: u16,
        pointer_x: i16,
        pointer_y: i16,
    },
    Screenshot {
        width: u16,
        height: u16,
        png_base64: String,
    },
    Applied,
}
impl Reply {
    pub fn validate(&self, s: &ScreenConfig) -> Result<(), AgentError> {
        let valid = match self {
            Self::Applied => true,
            Self::Info {
                width,
                height,
                pointer_x,
                pointer_y,
            } => *width == s.width && *height == s.height && s.point(*pointer_x, *pointer_y),
            Self::Screenshot {
                width,
                height,
                png_base64,
            } => {
                *width == s.width
                    && *height == s.height
                    && png_base64.len() <= SCREEN_BYTES.div_ceil(3) * 4
                    && STANDARD
                        .decode(png_base64)
                        .is_ok_and(|b| validate_png(&b, s).is_ok())
            }
        };
        if valid {
            Ok(())
        } else {
            Err(AgentError::Protocol("invalid guest desktop response"))
        }
    }
}
pub fn validate_png(bytes: &[u8], s: &ScreenConfig) -> Result<(), AgentError> {
    s.validate()?;
    if bytes.len() > SCREEN_BYTES {
        return Err(AgentError::Protocol("screenshot limit"));
    }
    let mut decoder = png::Decoder::new(bytes);
    decoder.set_limits(png::Limits {
        bytes: 16 * 1024 * 1024,
    });
    let mut reader = decoder
        .read_info()
        .map_err(|_| AgentError::Protocol("invalid screenshot PNG"))?;
    let i = reader.info();
    if i.width != u32::from(s.width)
        || i.height != u32::from(s.height)
        || i.color_type != png::ColorType::Rgb
        || i.bit_depth != png::BitDepth::Eight
        || i.animation_control.is_some()
    {
        return Err(AgentError::Protocol("unexpected screenshot format"));
    }
    let mut buffer = vec![0; reader.output_buffer_size()];
    reader
        .next_frame(&mut buffer)
        .map_err(|_| AgentError::Protocol("invalid screenshot pixels"))?;
    reader
        .finish()
        .map_err(|_| AgentError::Protocol("invalid screenshot trailer"))?;
    Ok(())
}
impl<E: crate::Engine> crate::Computer<'_, E> {
    pub fn desktop(&mut self, request: Request, ctx: &TurnContext) -> Result<Reply, AgentError> {
        ctx.remaining()?;
        let screen = self
            .snapshot()
            .spec
            .desktop
            .as_ref()
            .ok_or(AgentError::Configuration(
                "desktop must be explicitly configured",
            ))?
            .clone();
        request.validate(&screen)?;
        let expected = match &request {
            Request::Info => 0,
            Request::Screenshot => 1,
            _ => 2,
        };
        let reply = self.execute(
            crate::protocol::GuestRequest::Desktop {
                request,
                timeout_ms: ctx.remaining()?.as_millis().clamp(1, 15000) as u64,
            },
            ctx,
        )?;
        let crate::protocol::GuestValue::Desktop { value } = reply else {
            return Err(AgentError::Protocol("desktop response required"));
        };
        if !matches!(
            (expected, &value),
            (0, Reply::Info { .. }) | (1, Reply::Screenshot { .. }) | (2, Reply::Applied)
        ) {
            self.abort_remote()?;
            return Err(AgentError::Protocol("wrong desktop response kind"));
        }
        value.validate(&screen)?;
        Ok(value)
    }
}
