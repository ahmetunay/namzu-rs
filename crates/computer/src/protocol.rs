use namzu_tools::process::ProcessOutput;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path};
pub const FILE_LIMIT: usize = 64 * 1024;
pub const WIRE_LIMIT: usize = 512 * 1024;
pub const GUEST: &str = "/usr/local/bin/namzu-guest";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum GuestRequest {
    Browser {
        request: crate::browser::Request,
        timeout_ms: u64,
    },
    Desktop {
        request: crate::desktop::Request,
        timeout_ms: u64,
    },
    Probe,
    Read {
        path: String,
    },
    Write {
        path: String,
        base64: String,
    },
    List {
        path: String,
    },
    Shell {
        command: String,
        cwd: String,
        timeout_ms: u64,
        #[serde(
            default = "default_output_limit",
            skip_serializing_if = "is_default_output_limit"
        )]
        output_limit: usize,
    },
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GuestValue {
    Browser { value: crate::browser::Reply },
    Desktop { value: crate::desktop::Reply },
    Ready,
    Written,
    Bytes { base64: String },
    Entries { names: Vec<String> },
    Shell { output: ProcessOutput },
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuestReply {
    pub version: u32,
    pub result: Result<GuestValue, String>,
}

pub fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 1024
        && !path.contains('\0')
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

pub fn default_output_limit() -> usize {
    FILE_LIMIT
}
fn is_default_output_limit(value: &usize) -> bool {
    *value == FILE_LIMIT
}
