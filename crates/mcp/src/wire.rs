use crate::{Error, Limits};
use serde::{
    Serialize, Serializer,
    de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value, json};
use std::{fmt, io::Write};

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;
const SAFE_INTEGER: i64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum Id {
    Number(i64),
    String(String),
}
impl Id {
    fn parse(value: &Value) -> Result<Self, Error> {
        match value {
            Value::String(s)
                if !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control) =>
            {
                Ok(Self::String(s.clone()))
            }
            Value::Number(n) => n
                .as_i64()
                .filter(|n| (-SAFE_INTEGER..=SAFE_INTEGER).contains(n))
                .map(Self::Number)
                .ok_or(Error::Envelope("integer request ID required")),
            _ => Err(Error::Envelope(
                "bounded string or integer request ID required",
            )),
        }
    }
}
/// Error data is untrusted peer content. Never put message/data into automatic
/// logs or convert it to host instructions. Error Display above contains no body.
#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    Request {
        id: Id,
        method: String,
        params: Option<Map<String, Value>>,
    },
    Notification {
        method: String,
        params: Option<Map<String, Value>>,
    },
    Success {
        id: Id,
        result: Map<String, Value>,
    },
    /// Missing/null ID is allowed only on an uncorrelated error response.
    Failure {
        id: Option<Id>,
        error: RpcError,
    },
}
impl Serialize for Message {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut o = serializer.serialize_map(None)?;
        o.serialize_entry("jsonrpc", "2.0")?;
        match self {
            Self::Request { id, method, params } => {
                o.serialize_entry("id", id)?;
                o.serialize_entry("method", method)?;
                if let Some(p) = params {
                    o.serialize_entry("params", p)?;
                }
            }
            Self::Notification { method, params } => {
                o.serialize_entry("method", method)?;
                if let Some(p) = params {
                    o.serialize_entry("params", p)?;
                }
            }
            Self::Success { id, result } => {
                o.serialize_entry("id", id)?;
                o.serialize_entry("result", result)?;
            }
            Self::Failure { id, error } => {
                if let Some(id) = id {
                    o.serialize_entry("id", id)?;
                }
                o.serialize_entry("error", error)?;
            }
        }
        o.end()
    }
}
fn envelope(v: Value) -> Result<Message, Error> {
    let Value::Object(mut o) = v else {
        return Err(Error::Envelope(
            "single object required; batches unsupported",
        ));
    };
    if o.remove("jsonrpc") != Some(json!("2.0")) {
        return Err(Error::Envelope("jsonrpc must be 2.0"));
    }
    if let Some(method) = o.remove("method") {
        let method = method
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control))
            .ok_or(Error::Envelope("invalid method"))?
            .to_owned();
        let params = match o.remove("params") {
            None => None,
            Some(Value::Object(p)) => Some(p),
            _ => return Err(Error::Envelope("MCP params must be an object")),
        };
        let id = o.remove("id");
        if !o.is_empty() {
            return Err(Error::Envelope("unexpected request envelope field"));
        }
        return if let Some(id) = id {
            Ok(Message::Request {
                id: Id::parse(&id)?,
                method,
                params,
            })
        } else {
            Ok(Message::Notification { method, params })
        };
    }
    let id = o.remove("id");
    let result = o.remove("result");
    let error = o.remove("error");
    if !o.is_empty() {
        return Err(Error::Envelope("unexpected response envelope field"));
    }
    match (result, error) {
        (Some(Value::Object(result)), None) => Ok(Message::Success {
            id: Id::parse(id.as_ref().ok_or(Error::Envelope("success requires ID"))?)?,
            result,
        }),
        (None, Some(Value::Object(e))) => {
            if !e
                .keys()
                .all(|k| ["code", "message", "data"].contains(&k.as_str()))
                || e.get("code").and_then(Value::as_i64).is_none()
                || e.get("message").and_then(Value::as_str).is_none()
            {
                return Err(Error::Envelope("invalid error object"));
            }
            let error = serde_json::from_value(Value::Object(e))
                .map_err(|_| Error::Envelope("invalid error object"))?;
            Ok(Message::Failure {
                id: match id {
                    None | Some(Value::Null) => None,
                    Some(v) => Some(Id::parse(&v)?),
                },
                error,
            })
        }
        _ => Err(Error::Envelope(
            "exactly one object result or error required",
        )),
    }
}

struct Budget<'a> {
    limits: &'a Limits,
    nodes: usize,
}
struct Seed<'a, 'b> {
    budget: &'a mut Budget<'b>,
    depth: usize,
}
impl<'de> DeserializeSeed<'de> for Seed<'_, '_> {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        if self.depth > self.budget.limits.depth || self.budget.nodes >= self.budget.limits.nodes {
            return Err(de::Error::custom("JSON structural limit"));
        }
        self.budget.nodes += 1;
        d.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Seed<'_, '_> {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("bounded JSON")
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Value, E> {
        Ok(json!(v))
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Value, E> {
        Ok(json!(v))
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Value, E> {
        Ok(json!(v))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Value, E> {
        serde_json::Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| de::Error::custom("invalid number"))
    }
    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_none<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<Value, E> {
        if v.len() > self.budget.limits.string_bytes {
            return Err(de::Error::custom("string limit"));
        }
        Ok(Value::String(v.into()))
    }
    fn visit_string<E: de::Error>(self, v: String) -> Result<Value, E> {
        self.visit_str(&v)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Value, A::Error> {
        let mut v = vec![];
        while let Some(item) = a.next_element_seed(Seed {
            budget: self.budget,
            depth: self.depth + 1,
        })? {
            v.push(item);
        }
        Ok(Value::Array(v))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Value, A::Error> {
        let mut v = Map::new();
        while let Some(key) = a.next_key::<String>()? {
            if key.len() > self.budget.limits.string_bytes || v.contains_key(&key) {
                return Err(de::Error::custom("duplicate or excessive key"));
            }
            let value = a.next_value_seed(Seed {
                budget: self.budget,
                depth: self.depth + 1,
            })?;
            v.insert(key, value);
        }
        Ok(Value::Object(v))
    }
}
pub fn decode(bytes: &[u8], limits: &Limits) -> Result<Message, Error> {
    limits.validate()?;
    if bytes.len() > limits.message_bytes {
        return Err(Error::MessageLimit);
    }
    let mut d = serde_json::Deserializer::from_slice(bytes);
    let mut budget = Budget { limits, nodes: 0 };
    let value = Seed {
        budget: &mut budget,
        depth: 0,
    }
    .deserialize(&mut d)
    .map_err(|_| Error::Json)?;
    d.end().map_err(|_| Error::Json)?;
    envelope(value)
}
struct Writer {
    bytes: Vec<u8>,
    max: usize,
}
impl Write for Writer {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if b.len() > self.max - self.bytes.len() {
            return Err(std::io::Error::other("message limit"));
        }
        self.bytes.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn value(v: &Value, depth: usize, nodes: &mut usize, l: &Limits) -> Result<(), Error> {
    if depth > l.depth || *nodes >= l.nodes {
        return Err(Error::MessageLimit);
    }
    *nodes += 1;
    match v {
        Value::String(s) if s.len() > l.string_bytes => return Err(Error::MessageLimit),
        Value::Array(a) => {
            for v in a {
                value(v, depth + 1, nodes, l)?;
            }
        }
        Value::Object(o) => map(o, depth + 1, nodes, l)?,
        _ => (),
    }
    Ok(())
}
fn map(o: &Map<String, Value>, depth: usize, nodes: &mut usize, l: &Limits) -> Result<(), Error> {
    for (k, v) in o {
        if k.len() > l.string_bytes {
            return Err(Error::MessageLimit);
        }
        value(v, depth, nodes, l)?;
    }
    Ok(())
}
pub(crate) fn validate_metadata(o: &Map<String, Value>, limits: &Limits) -> Result<(), Error> {
    map(o, 0, &mut 0, limits)?;
    let mut writer = Writer {
        bytes: vec![],
        max: limits.message_bytes,
    };
    serde_json::to_writer(&mut writer, o).map_err(|_| Error::MessageLimit)?;
    Ok(())
}
/// The same decoder validates constructed outbound values too; no unchecked wire
/// serialization API is used by ClientSession. Byte allocation stays bounded.
pub fn encode(message: &Message, limits: &Limits) -> Result<Vec<u8>, Error> {
    limits.validate()?;
    // Preflight host-constructed Values before serde recursion/allocation. Incoming
    // payloads already passed the strict bounded parser, but SDK callers may build
    // arbitrary JSON trees themselves.
    let mut nodes = 0;
    match message {
        Message::Request {
            params: Some(p), ..
        }
        | Message::Notification {
            params: Some(p), ..
        } => map(p, 0, &mut nodes, limits)?,
        Message::Success { result, .. } => map(result, 0, &mut nodes, limits)?,
        Message::Failure {
            error: RpcError {
                data: Some(data), ..
            },
            ..
        } => value(data, 0, &mut nodes, limits)?,
        _ => (),
    }
    let mut writer = Writer {
        bytes: vec![],
        max: limits.message_bytes,
    };
    serde_json::to_writer(&mut writer, message).map_err(|_| Error::MessageLimit)?;
    decode(&writer.bytes, limits)?;
    Ok(writer.bytes)
}
