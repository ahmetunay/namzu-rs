//! Atomic bounded catalogs and typed MCP payload validation. No URI is opened.
use crate::{Id, Limits, Message, types::*};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("invalid MCP discovery payload")]
    Payload,
    #[error("MCP discovery resource limit exceeded")]
    Limit,
    #[error("MCP discovery cursor repeated or invalid")]
    Cursor,
    #[error("MCP discovery item identity is duplicated")]
    Duplicate,
    #[error("invalid or excessive MCP schema metadata")]
    Schema,
    #[error("MCP discovery catalogue changed or is not available")]
    Stale,
    #[error("MCP discovery RPC failed")]
    Rpc,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Tools,
    Resources,
    Prompts,
    Templates,
}
impl Kind {
    pub fn method(self) -> &'static str {
        match self {
            Self::Tools => "tools/list",
            Self::Resources => "resources/list",
            Self::Prompts => "prompts/list",
            Self::Templates => "resources/templates/list",
        }
    }
    fn field(self) -> &'static str {
        match self {
            Self::Tools => "tools",
            Self::Resources => "resources",
            Self::Prompts => "prompts",
            Self::Templates => "resourceTemplates",
        }
    }
    pub fn supported(self, cap: &ServerCapabilities) -> bool {
        match self {
            Self::Tools => cap.tools.is_some(),
            Self::Resources | Self::Templates => cap.resources.is_some(),
            Self::Prompts => cap.prompts.is_some(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", content = "definition", rename_all = "snake_case")]
pub enum Item {
    Tool(Tool),
    Resource(Resource),
    Prompt(Prompt),
    Template(ResourceTemplate),
}
impl Item {
    pub fn identity(&self) -> &str {
        match self {
            Self::Tool(v) => &v.name,
            Self::Resource(v) => &v.uri,
            Self::Prompt(v) => &v.name,
            Self::Template(v) => &v.uri_template,
        }
    }
    fn parse(kind: Kind, value: Value) -> Result<Self, Error> {
        fn parse<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, Error> {
            serde_json::from_value(v).map_err(|_| Error::Payload)
        }
        let object = value.as_object().ok_or(Error::Payload)?;
        non_null(object, &["description", "mimeType", "size", "outputSchema"])?;
        if kind == Kind::Prompts
            && let Some(arguments) = object.get("arguments").and_then(Value::as_array)
        {
            for argument in arguments {
                non_null(
                    argument.as_object().ok_or(Error::Payload)?,
                    &["description"],
                )?;
            }
        }
        let item = match kind {
            Kind::Tools => {
                let tool: Tool = parse(value)?;
                name(&tool.name)?;
                description(tool.description.as_deref())?;
                crate::schema::check(&tool.input_schema)?;
                if let Some(schema) = &tool.output_schema {
                    crate::schema::check(schema)?;
                }
                Self::Tool(tool)
            }
            Kind::Resources => {
                let resource: Resource = parse(value)?;
                uri(&resource.uri)?;
                label(&resource.name)?;
                description(resource.description.as_deref())?;
                mime(resource.mime_type.as_deref())?;
                Self::Resource(resource)
            }
            Kind::Prompts => {
                let prompt: Prompt = parse(value)?;
                name(&prompt.name)?;
                description(prompt.description.as_deref())?;
                if prompt.arguments.len() > 64 {
                    return Err(Error::Limit);
                }
                let mut names = BTreeSet::new();
                for arg in &prompt.arguments {
                    name(&arg.name)?;
                    description(arg.description.as_deref())?;
                    if !names.insert(&arg.name) {
                        return Err(Error::Duplicate);
                    }
                }
                Self::Prompt(prompt)
            }
            Kind::Templates => {
                let template: ResourceTemplate = parse(value)?;
                uri(&template.uri_template)?;
                template_syntax(&template.uri_template)?;
                label(&template.name)?;
                description(template.description.as_deref())?;
                mime(template.mime_type.as_deref())?;
                Self::Template(template)
            }
        };
        Ok(item)
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Catalog {
    pub server: String,
    pub kind: Kind,
    pub revision: u64,
    pub items: Vec<Item>,
}
/// Builder never exposes partial items. A failed refresh cannot publish a prefix.
pub struct Pager {
    kind: Kind,
    pages: usize,
    bytes: usize,
    items: Vec<Item>,
    identities: BTreeSet<String>,
    cursors: BTreeSet<String>,
    next: Option<String>,
    done: bool,
    failed: bool,
}
impl Pager {
    pub fn new(kind: Kind) -> Self {
        Self {
            kind,
            pages: 0,
            bytes: 0,
            items: vec![],
            identities: BTreeSet::new(),
            cursors: BTreeSet::new(),
            next: None,
            done: false,
            failed: false,
        }
    }
    pub fn request_params(&self) -> Result<Option<Map<String, Value>>, Error> {
        if self.failed
            || self.done
            || self.pages >= 32
            || self.bytes >= 4 * 1024 * 1024
            || self.items.len() >= 1024
        {
            return Err(Error::Limit);
        }
        Ok(self.params())
    }
    pub fn params(&self) -> Option<Map<String, Value>> {
        self.next
            .as_ref()
            .map(|cursor| Map::from_iter([("cursor".into(), Value::String(cursor.clone()))]))
    }
    pub fn push(&mut self, result: Map<String, Value>) -> Result<bool, Error> {
        if self.failed {
            return Err(Error::Stale);
        }
        self.failed = true;
        // Preflight host-constructed results before serde recursion/cloning.
        let message = Message::Success {
            id: Id::Number(1),
            result,
        };
        let bytes = crate::wire::encode(&message, &Limits::default())
            .map_err(|_| Error::Limit)?
            .len();
        let Message::Success { mut result, .. } = message else {
            unreachable!()
        };
        if self.done || self.pages >= 32 || self.bytes.saturating_add(bytes) > 4 * 1024 * 1024 {
            return Err(Error::Limit);
        }
        // Any failure poisons this builder; caller must explicitly start afresh.
        self.done = true;
        let next = match result.remove("nextCursor") {
            None => None,
            Some(Value::String(s))
                if !s.is_empty() && s.len() <= 1024 && !s.chars().any(char::is_control) =>
            {
                Some(s)
            }
            _ => return Err(Error::Cursor),
        };
        if let Some(cursor) = &next
            && !self.cursors.insert(cursor.clone())
        {
            return Err(Error::Cursor);
        }
        let values = result
            .remove(self.kind.field())
            .and_then(|v| v.as_array().cloned())
            .ok_or(Error::Payload)?;
        if self.items.len().saturating_add(values.len()) > 1024 {
            return Err(Error::Limit);
        }
        for value in values {
            let item = Item::parse(self.kind, value)?;
            if !self.identities.insert(item.identity().into()) {
                return Err(Error::Duplicate);
            }
            self.items.push(item);
        }
        self.pages += 1;
        self.bytes += bytes;
        self.next = next;
        self.done = self.next.is_none();
        self.failed = false;
        Ok(self.done)
    }
    pub fn finish(self, server: String, revision: u64) -> Result<Catalog, Error> {
        if self.failed
            || self.pages == 0
            || !self.done
            || self.next.is_some()
            || !crate::config::valid_name(&server)
        {
            return Err(Error::Stale);
        }
        Ok(Catalog {
            server,
            kind: self.kind,
            revision,
            items: self.items,
        })
    }
}
fn name(value: &str) -> Result<(), Error> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    {
        Err(Error::Payload)
    } else {
        Ok(())
    }
}
fn label(value: &str) -> Result<(), Error> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        Err(Error::Payload)
    } else {
        Ok(())
    }
}
fn description(value: Option<&str>) -> Result<(), Error> {
    if value.is_some_and(|v| v.len() > 16384) {
        Err(Error::Limit)
    } else {
        Ok(())
    }
}
fn mime(value: Option<&str>) -> Result<(), Error> {
    if value.is_some_and(|v| v.is_empty() || v.len() > 256 || v.chars().any(char::is_control)) {
        Err(Error::Payload)
    } else {
        Ok(())
    }
}
pub fn uri(value: &str) -> Result<(), Error> {
    let (scheme, _) = value.split_once(':').ok_or(Error::Payload)?;
    if value.len() > 4096
        || value.chars().any(|c| c.is_control() || c.is_whitespace())
        || !scheme
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic())
        || !scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+.-".contains(&b))
    {
        return Err(Error::Payload);
    }
    Ok(())
}
fn non_null(object: &Map<String, Value>, fields: &[&str]) -> Result<(), Error> {
    if fields
        .iter()
        .any(|f| object.get(*f).is_some_and(Value::is_null))
    {
        Err(Error::Payload)
    } else {
        Ok(())
    }
}
fn template_syntax(value: &str) -> Result<(), Error> {
    let mut expression = false;
    let mut count = 0;
    for c in value.chars() {
        match c {
            '{' if !expression => {
                expression = true;
                count = 0;
            }
            '}' if expression && count > 0 => expression = false,
            '{' | '}' => return Err(Error::Payload),
            _ if expression => count += 1,
            _ => (),
        }
    }
    if expression {
        Err(Error::Payload)
    } else {
        Ok(())
    }
}
fn preflight(result: &Map<String, Value>) -> Result<(), Error> {
    crate::wire::validate_metadata(result, &Limits::default()).map_err(|_| Error::Limit)
}
pub fn read_result(result: Map<String, Value>) -> Result<ReadResourceResult, Error> {
    preflight(&result)?;
    if let Some(contents) = result.get("contents").and_then(Value::as_array) {
        for content in contents {
            non_null(
                content.as_object().ok_or(Error::Payload)?,
                &["text", "blob", "mimeType"],
            )?;
        }
    }
    let result: ReadResourceResult =
        serde_json::from_value(Value::Object(result)).map_err(|_| Error::Payload)?;
    if result.contents.len() > 128 {
        return Err(Error::Limit);
    }
    for content in &result.contents {
        uri(&content.uri)?;
        mime(content.mime_type.as_deref())?;
        if content.text.is_some() == content.blob.is_some() {
            return Err(Error::Payload);
        }
    }
    Ok(result)
}
pub fn prompt_result(result: Map<String, Value>) -> Result<GetPromptResult, Error> {
    preflight(&result)?;
    non_null(&result, &["description"])?;
    let result: GetPromptResult =
        serde_json::from_value(Value::Object(result)).map_err(|_| Error::Payload)?;
    description(result.description.as_deref())?;
    if result.messages.len() > 128 {
        return Err(Error::Limit);
    }
    for message in &result.messages {
        if !["user", "assistant"].contains(&message.role.as_str()) {
            return Err(Error::Payload);
        }
        content(&message.content)?;
    }
    Ok(result)
}
pub fn tool_result(result: Map<String, Value>) -> Result<CallToolResult, Error> {
    preflight(&result)?;
    non_null(&result, &["structuredContent"])?;
    let result: CallToolResult =
        serde_json::from_value(Value::Object(result)).map_err(|_| Error::Payload)?;
    if result.content.len() > 128 {
        return Err(Error::Limit);
    }
    for item in &result.content {
        content(item)?;
    }
    Ok(result)
}
fn content(value: &Content) -> Result<(), Error> {
    match value {
        Content::Text { .. } => Ok(()),
        Content::Image { mime_type, .. } | Content::Audio { mime_type, .. } => {
            mime(Some(mime_type))
        }
        Content::ResourceLink {
            uri: value, name, ..
        } => {
            uri(value)?;
            label(name)
        }
        Content::Resource { resource, .. } => {
            let result = read_result(Map::from_iter([(
                "contents".into(),
                Value::Array(vec![Value::Object(resource.clone())]),
            )]))?;
            if result.contents.is_empty() {
                Err(Error::Payload)
            } else {
                Ok(())
            }
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Notification {
    pub server: String,
    pub generation: u64,
    pub method: String,
    pub params: Option<Map<String, Value>>,
}
