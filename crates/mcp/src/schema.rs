//! Bounded JSON Schema metadata admission, without network or instance evaluation.
//! Complex constraints remain data for the future model/tool execution adapter.
use crate::discovery::Error;
use serde_json::{Map, Value};
use std::collections::BTreeSet;

pub fn check(root: &Map<String, Value>) -> Result<(), Error> {
    let limits = crate::Limits {
        message_bytes: 65536,
        string_bytes: 16384,
        depth: 24,
        nodes: 4096,
        ..crate::Limits::default()
    };
    crate::wire::validate_metadata(root, &limits).map_err(|_| Error::Schema)?;
    if root.get("type").and_then(Value::as_str) != Some("object") {
        return Err(Error::Schema);
    }
    let root = Value::Object(root.clone());
    node(&root, &root)
}
fn node(value: &Value, root: &Value) -> Result<(), Error> {
    if value.is_boolean() {
        return Ok(());
    }
    let object = value.as_object().ok_or(Error::Schema)?;
    for (key, value) in object {
        match key.as_str() {
            "type" => {
                let valid = |s: &str| {
                    [
                        "object", "array", "string", "integer", "number", "boolean", "null",
                    ]
                    .contains(&s)
                };
                if let Some(t) = value.as_str() {
                    if !valid(t) {
                        return Err(Error::Schema);
                    }
                } else {
                    let types = value
                        .as_array()
                        .filter(|a| !a.is_empty() && a.len() <= 7)
                        .ok_or(Error::Schema)?;
                    let mut seen = BTreeSet::new();
                    for t in types {
                        let t = t.as_str().filter(|s| valid(s)).ok_or(Error::Schema)?;
                        if !seen.insert(t) {
                            return Err(Error::Schema);
                        }
                    }
                }
            }
            "$ref" | "$dynamicRef" | "$recursiveRef" => {
                let reference = value.as_str().ok_or(Error::Schema)?;
                let pointer = reference
                    .strip_prefix('#')
                    .filter(|p| p.is_empty() || p.starts_with('/'))
                    .ok_or(Error::Schema)?;
                let target = root
                    .pointer(pointer)
                    .filter(|t| t.is_object() || t.is_boolean())
                    .ok_or(Error::Schema)?;
                let _ = target; // Do not expand recursive schemas or fetch a URI.
            }
            "$schema" | "$id" | "$anchor" | "title" | "description" | "format" | "pattern"
            | "contentMediaType" | "contentEncoding" => {
                if !value.is_string() {
                    return Err(Error::Schema);
                }
            }
            "required" => {
                let items = value.as_array().ok_or(Error::Schema)?;
                let mut seen = BTreeSet::new();
                for item in items {
                    let item = item
                        .as_str()
                        .filter(|s| s.len() <= 1024 && !s.chars().any(char::is_control))
                        .ok_or(Error::Schema)?;
                    if !seen.insert(item) {
                        return Err(Error::Schema);
                    }
                }
            }
            "properties" | "patternProperties" | "$defs" | "definitions" | "dependentSchemas" => {
                for schema in value.as_object().ok_or(Error::Schema)?.values() {
                    node(schema, root)?;
                }
            }
            "additionalProperties"
            | "unevaluatedProperties"
            | "unevaluatedItems"
            | "propertyNames"
            | "contains"
            | "not"
            | "if"
            | "then"
            | "else" => node(value, root)?,
            "items" => {
                if let Some(items) = value.as_array() {
                    for item in items {
                        node(item, root)?;
                    }
                } else {
                    node(value, root)?;
                }
            }
            "allOf" | "anyOf" | "oneOf" | "prefixItems" => {
                let items = value
                    .as_array()
                    .filter(|a| !a.is_empty() && a.len() <= 64)
                    .ok_or(Error::Schema)?;
                for item in items {
                    node(item, root)?;
                }
            }
            "enum" => {
                let values = value
                    .as_array()
                    .filter(|a| !a.is_empty() && a.len() <= 256)
                    .ok_or(Error::Schema)?;
                for (i, v) in values.iter().enumerate() {
                    if values[..i].contains(v) {
                        return Err(Error::Schema);
                    }
                }
            }
            "minLength" | "maxLength" | "minItems" | "maxItems" | "minProperties"
            | "maxProperties" | "minContains" | "maxContains" => {
                if value.as_u64().is_none() {
                    return Err(Error::Schema);
                }
            }
            "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum" => {
                if !value.is_number() {
                    return Err(Error::Schema);
                }
            }
            "multipleOf" => {
                if value.as_f64().is_none_or(|n| n <= 0.0) {
                    return Err(Error::Schema);
                }
            }
            "uniqueItems" | "readOnly" | "writeOnly" | "deprecated" if !value.is_boolean() => {
                return Err(Error::Schema);
            }
            _ => (), // Extensions/annotations retained, never treated as permissions.
        }
    }
    Ok(())
}
