//! Explicit bounded model discovery and private immutable catalogue selection.
//! Reported limits are metadata, not an inferred tokenizer or access/free-tier guarantee.
use crate::{OpenAiCompatible, services::Service};
use namzu_core::{AgentError, ProviderInfo, TurnContext};
use namzu_session::PrivateStore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, io::Read};
const MODELS: usize = 4096;
fn bad() -> AgentError {
    AgentError::Protocol("invalid or excessive model catalogue")
}
fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control)
}
fn hash(v: &impl Serialize) -> Result<String, AgentError> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(v).map_err(|_| bad())?)
    ))
}
pub fn canonical_base(s: &str) -> Result<String, AgentError> {
    let mut u = url::Url::parse(s).map_err(|_| bad())?;
    if !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
        || u.host_str().is_none()
    {
        return Err(bad());
    }
    let path = u.path().trim_end_matches('/').to_owned();
    u.set_path(&path);
    Ok(u.to_string())
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Model {
    pub id: String,
    pub name: Option<String>,
    pub aliases: Vec<String>,
    pub context_window: Option<u64>,
    pub input_token_limit: Option<u64>,
    pub output_token_limit: Option<u64>,
    pub tools: Option<bool>,
    pub images: Option<bool>,
}
impl Model {
    pub fn validate(&self) -> Result<(), AgentError> {
        if !valid_id(&self.id)
            || self.aliases.len() > 8
            || self.aliases.iter().any(|s| !valid_id(s))
            || self
                .name
                .as_ref()
                .is_some_and(|s| s.len() > 512 || s.chars().any(char::is_control))
            || [
                self.context_window,
                self.input_token_limit,
                self.output_token_limit,
            ]
            .into_iter()
            .flatten()
            .any(|n| n == 0 || n > 1_000_000_000)
        {
            return Err(bad());
        }
        Ok(())
    }
    pub fn matches(&self, id: &str) -> bool {
        self.id == id || self.aliases.iter().any(|v| v == id)
    }
    /// Caller-supplied measured input count. No tokenizer is guessed by this SDK.
    pub fn validate_tokens(&self, input: Option<u64>, output: u64) -> Result<(), AgentError> {
        self.validate()?;
        if output == 0
            || self.output_token_limit.is_some_and(|n| output > n)
            || self.context_window.is_some_and(|n| output > n)
            || input
                .zip(self.input_token_limit)
                .is_some_and(|(a, b)| a > b)
            || input
                .zip(self.context_window)
                .is_some_and(|(a, b)| a.checked_add(output).is_none_or(|n| n > b))
        {
            return Err(AgentError::Configuration(
                "selected model token limit exceeded",
            ));
        }
        Ok(())
    }
    pub fn output_budget(&self, explicit: Option<u32>) -> Result<u32, AgentError> {
        let n = explicit.map(u64::from).unwrap_or_else(|| {
            4096.min(self.output_token_limit.unwrap_or(4096))
                .min(self.context_window.unwrap_or(4096))
        });
        self.validate_tokens(None, n)?;
        u32::try_from(n).map_err(|_| bad())
    }
    pub(crate) fn apply_capabilities(&self, info: &mut ProviderInfo) {
        use namzu_core::provider::Support;
        if self.tools == Some(false) {
            info.capabilities.tools = Support::Unsupported;
        }
        if self.images == Some(false) {
            info.capabilities.images = Support::Unsupported;
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub version: u32,
    pub provider: String,
    pub base_url: String,
    pub captured_ms: u64,
    pub models: Vec<Model>,
}
impl Catalog {
    pub fn validate(&self) -> Result<(), AgentError> {
        if self.version != 1
            || self.models.len() > MODELS
            || self.provider != "codex" && Service::parse(&self.provider).is_err()
        {
            return Err(bad());
        }
        canonical_base(&self.base_url)?;
        let mut seen = HashSet::new();
        for m in &self.models {
            m.validate()?;
            for id in std::iter::once(&m.id).chain(m.aliases.iter()) {
                if !seen.insert(id) {
                    return Err(bad());
                }
            }
        }
        Ok(())
    }
    pub fn select(&self, id: &str) -> Result<&Model, AgentError> {
        self.validate()?;
        let id = if self.provider == "codex" {
            id
        } else {
            Service::parse(&self.provider)?.normalize_model(id)
        };
        self.models
            .iter()
            .find(|m| m.matches(id))
            .ok_or(AgentError::Configuration(
                "selected model is absent from the chosen catalogue; no fallback is performed",
            ))
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cached {
    pub catalog: Catalog,
    pub selector: String,
    binding: Value,
}
impl std::fmt::Debug for Cached {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cached")
            .field("catalog", &self.catalog)
            .field("selector", &self.selector)
            .finish_non_exhaustive()
    }
}
impl Cached {
    fn digest(&self) -> Result<String, AgentError> {
        hash(&(
            self.catalog.version,
            &self.catalog.provider,
            &self.catalog.base_url,
            &self.catalog.models,
            &self.selector,
            &self.binding,
        ))
    }
    pub fn valid_id(id: &str) -> bool {
        id.len() == 66
            && id.starts_with("c-")
            && id[2..]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
    pub fn read(store: &PrivateStore, id: &str) -> Result<Self, AgentError> {
        if !Self::valid_id(id) {
            return Err(AgentError::Configuration("invalid catalogue ID"));
        }
        let value: Self = store
            .open_child("catalogs")?
            .read(&format!("{id}.json"))?
            .ok_or(AgentError::State("selected catalogue not found"))?;
        value.catalog.validate()?;
        if format!("c-{}", value.digest()?) != id {
            return Err(AgentError::State(
                "catalogue digest differs; refresh it explicitly",
            ));
        }
        Ok(value)
    }
    pub fn require_route(
        &self,
        provider: &str,
        base: &str,
        selector: &str,
    ) -> Result<(), AgentError> {
        if self.catalog.provider != provider
            || canonical_base(base)? != canonical_base(&self.catalog.base_url)?
            || self.selector != selector
        {
            return Err(AgentError::Configuration(
                "catalogue provider/endpoint/key selector differs from selected route",
            ));
        }
        Ok(())
    }
    pub(crate) fn verify_binding(&self, binding: &Value) -> Result<(), AgentError> {
        if &self.binding != binding {
            return Err(AgentError::Configuration(
                "catalogue account/credential/region changed; refresh explicitly",
            ));
        }
        Ok(())
    }
    pub(crate) fn save(
        store: &PrivateStore,
        catalog: Catalog,
        selector: String,
        binding: Value,
    ) -> Result<String, AgentError> {
        catalog.validate()?;
        if selector.is_empty() || selector.len() > 256 {
            return Err(bad());
        }
        let entry = Self {
            catalog,
            selector,
            binding,
        };
        let id = format!("c-{}", entry.digest()?);
        let child = store.child("catalogs")?;
        let _lock = child.lock("catalogue")?;
        child.write(&format!("{id}.json"), &entry)?;
        Ok(id)
    }
}
fn number(v: &Value, pointer: &str) -> Result<Option<u64>, AgentError> {
    match v.pointer(pointer) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .filter(|n| *n > 0 && *n <= 1_000_000_000)
            .map(Some)
            .ok_or_else(bad),
    }
}
fn boolean(v: &Value, pointer: &str) -> Result<Option<bool>, AgentError> {
    match v.pointer(pointer) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_bool().map(Some).ok_or_else(bad),
    }
}
fn string(v: &Value, key: &str) -> Result<Option<String>, AgentError> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_str().map(|s| Some(s.to_owned())).ok_or_else(bad),
    }
}
fn min(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}
fn list_has(v: &Value, pointer: &str, needle: &str) -> Result<Option<bool>, AgentError> {
    let Some(v) = v.pointer(pointer).filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let a = v.as_array().filter(|a| a.len() <= 512).ok_or_else(bad)?;
    if a.iter().any(|v| {
        v.as_str()
            .is_none_or(|s| s.len() > 128 || s.chars().any(char::is_control))
    }) {
        return Err(bad());
    }
    Ok(Some(a.iter().any(|v| v == needle)))
}
fn model(v: &Value, service: &str) -> Result<Model, AgentError> {
    let (id, name) = match service {
        "google" => (
            string(v, "name")?.map(|s| s.strip_prefix("models/").unwrap_or(&s).to_owned()),
            string(v, "displayName")?,
        ),
        "ollama" => (
            string(v, "model")?.or(string(v, "name")?),
            string(v, "name")?,
        ),
        "bedrock" => (string(v, "modelId")?, string(v, "modelName")?),
        "codex" => (string(v, "slug")?, string(v, "display_name")?),
        _ => (
            string(v, "id")?,
            string(v, "display_name")?.or(string(v, "name")?),
        ),
    };
    let mut result = Model {
        id: id.ok_or_else(bad)?,
        name,
        ..Default::default()
    };
    result.context_window = min(number(v, "/context_length")?, number(v, "/context_window")?);
    result.context_window = min(
        result.context_window,
        number(v, "/top_provider/context_length")?,
    );
    result.context_window = min(result.context_window, number(v, "/limits/context")?);
    result.context_window = min(result.context_window, number(v, "/max_context_length")?);
    result.input_token_limit = number(v, "/inputTokenLimit")?.or(number(v, "/max_input_tokens")?);
    result.output_token_limit = min(
        number(v, "/outputTokenLimit")?,
        number(v, "/max_output_tokens")?,
    );
    result.output_token_limit = min(
        result.output_token_limit,
        number(v, "/top_provider/max_completion_tokens")?,
    );
    result.output_token_limit = min(result.output_token_limit, number(v, "/limits/output")?);
    if service == "anthropic" {
        result.output_token_limit = min(result.output_token_limit, number(v, "/max_tokens")?);
    }
    result.tools =
        boolean(v, "/capabilities/tools")?.or(list_has(v, "/supported_parameters", "tools")?);
    result.images = boolean(v, "/capabilities/vision")?.or(list_has(
        v,
        "/architecture/input_modalities",
        "image",
    )?);
    if service == "bedrock" {
        result.images = result.images.or(list_has(v, "/inputModalities", "IMAGE")?);
        if let Some(arn) = string(v, "modelArn")?.filter(|a| a != &result.id) {
            result.aliases.push(arn);
        }
    }
    if service == "codex" {
        result.images = result.images.or(list_has(v, "/input_modalities", "image")?);
    }
    result.validate()?;
    Ok(result)
}
fn rows<'a>(v: &'a Value, service: &str) -> Result<&'a Vec<Value>, AgentError> {
    v[match service {
        "google" => "models",
        "ollama" => "models",
        "bedrock" => "modelSummaries",
        "codex" => "models",
        _ => "data",
    }]
    .as_array()
    .filter(|a| a.len() <= MODELS)
    .ok_or_else(bad)
}
fn next(v: &Value, service: &str) -> Result<Option<String>, AgentError> {
    let key = match service {
        "google" => "nextPageToken",
        "bedrock" => "nextToken",
        _ => "last_id",
    };
    let more = if service == "google" || service == "bedrock" {
        v.get(key).is_some_and(|v| !v.is_null() && v != "")
    } else {
        match v.get("has_more") {
            None | Some(Value::Null) => false,
            Some(v) => v.as_bool().ok_or_else(bad)?,
        }
    };
    if !more {
        return Ok(None);
    }
    let token = v[key]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 2048 && !s.chars().any(char::is_control))
        .ok_or_else(bad)?;
    if !["google", "anthropic"].contains(&service) {
        return Err(AgentError::Protocol(
            "pagination not implemented for selected catalogue",
        ));
    }
    Ok(Some(token.into()))
}
pub(crate) fn codex_catalog(value: &Value, base: &str) -> Result<Catalog, AgentError> {
    let mut models = vec![];
    for v in rows(value, "codex")? {
        if !matches!(v["visibility"].as_str(), Some("hide" | "hidden")) {
            models.push(model(v, "codex")?);
        }
    }
    let catalog = Catalog {
        version: 1,
        provider: "codex".into(),
        base_url: canonical_base(base)?,
        captured_ms: namzu_session::now_ms(),
        models,
    };
    catalog.validate()?;
    Ok(catalog)
}
impl OpenAiCompatible {
    pub(crate) fn catalog_binding(&self) -> Value {
        json!({"provider":self.service,"base_url":canonical_base(self.base_url.as_str()).unwrap(),"credential_tag":self.credential_tag,"auth":self.options.bedrock_auth.as_deref().unwrap_or("bearer"),"region":self.options.aws_region})
    }
    pub fn save_catalog(
        &self,
        store: &PrivateStore,
        catalog: Catalog,
        selector: &str,
    ) -> Result<String, AgentError> {
        if catalog.provider != self.service.name()
            || canonical_base(&catalog.base_url)? != canonical_base(self.base_url.as_str())?
        {
            return Err(bad());
        }
        Cached::save(store, catalog, selector.into(), self.catalog_binding())
    }
    pub fn require_catalog(&self, cached: &Cached, selector: &str) -> Result<(), AgentError> {
        cached.require_route(self.service.name(), self.base_url.as_str(), selector)?;
        cached.verify_binding(&self.catalog_binding())
    }
    pub fn with_model(mut self, model: Model) -> Result<Self, AgentError> {
        model.validate()?;
        if !model.matches(&self.model) {
            return Err(AgentError::Configuration(
                "catalogue record does not match selected model",
            ));
        }
        self.options.max_output_tokens = Some(model.output_budget(self.options.max_output_tokens)?);
        self.selected_model = Some(model);
        Ok(self)
    }
    pub fn model_metadata(&self) -> Option<&Model> {
        self.selected_model.as_ref()
    }
    /// Explicit model listing only. No inference, redirects, retry or stale-cache fallback.
    pub fn discover_models(&self, ctx: &TurnContext) -> Result<Catalog, AgentError> {
        let service = self.service.name();
        let mut all = vec![];
        let mut cursor: Option<String> = None;
        let mut seen = HashSet::new();
        let mut total = 0;
        for page in 0..32 {
            ctx.remaining()?;
            let mut url = self.base_url.clone();
            if self.service == Service::Bedrock
                && let Some(host) = url
                    .host_str()
                    .and_then(|h| h.strip_prefix("bedrock-runtime."))
                    .filter(|h| h.ends_with(".amazonaws.com"))
                    .map(|h| format!("bedrock.{h}"))
            {
                url.set_host(Some(&host)).map_err(|_| bad())?;
            }
            {
                let mut path = url.path_segments_mut().map_err(|_| bad())?;
                path.pop_if_empty();
                match self.service {
                    Service::Ollama => {
                        path.push("api").push("tags");
                    }
                    Service::Bedrock => {
                        path.push("foundation-models");
                    }
                    _ => {
                        path.push("models");
                    }
                }
            }
            if self.service == Service::Anthropic {
                url.query_pairs_mut().append_pair("limit", "1000");
            }
            if self.service == Service::Google {
                url.query_pairs_mut().append_pair("pageSize", "1000");
            }
            if let Some(token) = &cursor {
                url.query_pairs_mut().append_pair(
                    if self.service == Service::Google {
                        "pageToken"
                    } else {
                        "after_id"
                    },
                    token,
                );
            }
            let mut request = self.client.get(url.clone()).timeout(ctx.remaining()?);
            if let Some(aws) = &self.aws {
                request =
                    request.headers(aws.headers_for("GET", &url, b"", &crate::aws::now()?)?);
            } else if self.service == Service::Bedrock
                && self.options.bedrock_auth.as_deref() == Some("sigv4")
            {
                return Err(AgentError::Configuration(
                    "catalogue SigV4 credentials required",
                ));
            } else if let Some(auth) = &self.authorization {
                let header = match self.service {
                    Service::Anthropic => "x-api-key",
                    Service::Google => "x-goog-api-key",
                    _ => "authorization",
                };
                let mut auth = auth.clone();
                if matches!(self.service, Service::Zen | Service::ZenGo)
                    && matches!(
                        self.format,
                        crate::services::Format::Messages | crate::services::Format::Google
                    )
                {
                    auth = reqwest::header::HeaderValue::from_str(&format!(
                        "Bearer {}",
                        auth.to_str().map_err(|_| bad())?
                    ))
                    .map_err(|_| bad())?;
                    auth.set_sensitive(true);
                }
                request = request.header(header, auth);
            }
            if self.service == Service::Anthropic {
                request = request.header("anthropic-version", "2023-06-01");
            }
            let response = self.client.send(request, ctx)?;
            if !response.status().is_success() {
                return Err(crate::contract::status(&response));
            }
            let mut bytes = vec![];
            response
                .take(4 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| crate::http::read_error(ctx))?;
            ctx.remaining()?;
            total += bytes.len();
            if bytes.len() > 4 * 1024 * 1024 || total > 16 * 1024 * 1024 {
                return Err(bad());
            }
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| bad())?;
            if let Some(e) = crate::services::wire_error(&value) {
                return Err(e);
            }
            for row in rows(&value, service)? {
                if service == "google"
                    && list_has(row, "/supportedGenerationMethods", "generateContent")?
                        == Some(false)
                {
                    continue;
                }
                all.push(model(row, service)?);
                if all.len() > MODELS {
                    return Err(bad());
                }
            }
            cursor = next(&value, service)?;
            if let Some(token) = &cursor {
                if page == 31 || !seen.insert(token.clone()) {
                    return Err(bad());
                }
            } else {
                let catalog = Catalog {
                    version: 1,
                    provider: service.into(),
                    base_url: canonical_base(self.base_url.as_str())?,
                    captured_ms: namzu_session::now_ms(),
                    models: all,
                };
                catalog.validate()?;
                ctx.remaining()?;
                return Ok(catalog);
            }
        }
        Err(bad())
    }
}
#[cfg(test)]
mod tests;
