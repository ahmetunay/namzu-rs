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

/// Offline Draft 2020-12 validator. External retrieval is explicitly forbidden.
pub struct Compiled(jsonschema::Validator);
struct Offline;
impl jsonschema::Retrieve for Offline {
    fn retrieve(
        &self,
        _: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Err("external schema retrieval disabled".into())
    }
}
impl Compiled {
    pub fn new(schema: &Map<String, Value>) -> Result<Self, Error> {
        check(schema)?;
        if schema
            .get("$schema")
            .and_then(Value::as_str)
            .is_some_and(|s| s != "https://json-schema.org/draft/2020-12/schema")
        {
            return Err(Error::Schema);
        }
        let value = Value::Object(schema.clone());
        reference_safety(&value)?;
        let validator = jsonschema::options()
            .with_draft(jsonschema::Draft::Draft202012)
            .with_retriever(Offline)
            .with_pattern_options(jsonschema::PatternOptions::fancy_regex().backtrack_limit(10000))
            .build(&value)
            .map_err(|_| Error::Schema)?;
        Ok(Self(validator))
    }
    pub fn validate(
        &self,
        value: &Value,
        context: &namzu_core::TurnContext,
    ) -> Result<(), namzu_core::AgentError> {
        context.remaining()?;
        // Bound host constructed instances before traversing/validating them.
        let object = value.as_object().ok_or(namzu_core::AgentError::Protocol(
            "MCP object instance required",
        ))?;
        crate::wire::validate_metadata(object, &crate::Limits::default())
            .map_err(|_| namzu_core::AgentError::Protocol("MCP instance exceeds limits"))?;
        let valid = self.0.is_valid(value);
        context.remaining()?;
        if valid {
            Ok(())
        } else {
            Err(namzu_core::AgentError::Protocol(
                "MCP instance violates JSON Schema",
            ))
        }
    }
}

// A reference cycle that never consumes an instance property/item can recurse
// forever. Reject it before entering the validator; productive recursive object
// schemas remain usable under the wire instance depth limit.
fn reference_safety(root: &Value) -> Result<(), Error> {
    fn same<'a>(
        v: &'a Value,
        root: &'a Value,
        active: &mut std::collections::BTreeSet<usize>,
        budget: &mut usize,
    ) -> Result<(), Error> {
        if *budget == 0 {
            return Err(Error::Schema);
        }
        *budget -= 1;
        let Some(o) = v.as_object() else {
            return Ok(());
        };
        let id = v as *const Value as usize;
        if !active.insert(id) {
            return Err(Error::Schema);
        }
        for key in ["$ref", "$dynamicRef", "$recursiveRef"] {
            if let Some(r) = o.get(key).and_then(Value::as_str) {
                same(
                    root.pointer(r.strip_prefix('#').ok_or(Error::Schema)?)
                        .ok_or(Error::Schema)?,
                    root,
                    active,
                    budget,
                )?;
            }
        }
        for key in ["allOf", "anyOf", "oneOf"] {
            if let Some(a) = o.get(key).and_then(Value::as_array) {
                for child in a {
                    same(child, root, active, budget)?;
                }
            }
        }
        for key in ["not", "if", "then", "else"] {
            if let Some(child) = o.get(key) {
                same(child, root, active, budget)?;
            }
        }
        for key in ["dependentSchemas"] {
            if let Some(m) = o.get(key).and_then(Value::as_object) {
                for child in m.values() {
                    same(child, root, active, budget)?;
                }
            }
        }
        active.remove(&id);
        Ok(())
    }
    fn walk(v: &Value, root: &Value, budget: &mut usize) -> Result<(), Error> {
        same(v, root, &mut Default::default(), budget)?;
        let Some(o) = v.as_object() else {
            return Ok(());
        };
        for key in [
            "properties",
            "patternProperties",
            "$defs",
            "definitions",
            "dependentSchemas",
        ] {
            if let Some(m) = o.get(key).and_then(Value::as_object) {
                for child in m.values() {
                    walk(child, root, budget)?;
                }
            }
        }
        for key in [
            "items",
            "additionalProperties",
            "unevaluatedProperties",
            "unevaluatedItems",
            "contains",
            "propertyNames",
            "not",
            "if",
            "then",
            "else",
        ] {
            if let Some(child) = o.get(key) {
                if let Some(a) = child.as_array() {
                    for child in a {
                        walk(child, root, budget)?;
                    }
                } else {
                    walk(child, root, budget)?;
                }
            }
        }
        for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
            if let Some(a) = o.get(key).and_then(Value::as_array) {
                for child in a {
                    walk(child, root, budget)?;
                }
            }
        }
        Ok(())
    }
    walk(root, root, &mut 65536)
}
