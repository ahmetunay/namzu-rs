use crate::{
    Applied, Document, EditPlan, Position, Range, Spec, TextEdit, Workspace, digest, offset,
    transport::{CloseReport, Transport},
    workspace::bad,
};
use namzu_core::{AgentError, TurnContext};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub range: Range,
    pub severity: Option<u8>,
    pub code: Option<Value>,
    pub source: Option<String>,
    pub message: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Location {
    pub uri: String,
    pub range: Range,
}
pub struct Client {
    workspace: Workspace,
    io: Transport,
    lifetime: TurnContext,
    next: u64,
    received: usize,
    capabilities: Value,
    documents: BTreeMap<String, Document>,
    diagnostics: BTreeMap<String, Vec<Diagnostic>>,
    closed: bool,
    thread_bound: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl Client {
    pub fn start(spec: Spec, ctx: &TurnContext) -> Result<Self, AgentError> {
        ctx.remaining()?;
        spec.validate()?;
        let workspace = Workspace::new(&spec.cwd)?;
        let mut client = Self {
            workspace,
            io: Transport::spawn(&spec)?,
            lifetime: ctx.clone(),
            next: 0,
            received: 0,
            capabilities: Value::Null,
            documents: BTreeMap::new(),
            diagnostics: BTreeMap::new(),
            closed: false,
            thread_bound: std::marker::PhantomData,
        };
        let initialized=client.request("initialize",json!({"processId":null,"clientInfo":{"name":"namzu-rs","version":"0.1.0"},"rootUri":client.workspace.root_uri(),"workspaceFolders":[{"uri":client.workspace.root_uri(),"name":"workspace"}],"capabilities":{"general":{"positionEncodings":["utf-16"]},"workspace":{"applyEdit":false,"configuration":true,"workspaceEdit":{"documentChanges":true,"resourceOperations":[]}},"textDocument":{"publishDiagnostics":{"versionSupport":true},"diagnostic":{"dynamicRegistration":false}}}}),ctx)?;
        let capabilities = initialized
            .get("capabilities")
            .filter(|v| v.is_object())
            .ok_or(bad("invalid LSP initialize result"))?
            .clone();
        if capabilities
            .get("positionEncoding")
            .is_some_and(|v| v != "utf-16")
        {
            return Err(bad("LSP server must use UTF-16 positions"));
        }
        client.capabilities = capabilities;
        client.notify("initialized", json!({}), ctx)?;
        Ok(client)
    }
    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }
    pub fn capabilities(&self) -> &Value {
        &self.capabilities
    }
    fn guard(&self, ctx: &TurnContext) -> Result<(), AgentError> {
        ctx.remaining()?;
        self.lifetime.remaining()?;
        if self.closed || !ctx.is_bounded_by(&self.lifetime) {
            return Err(bad("closed or unbounded LSP context"));
        }
        Ok(())
    }
    fn notify(&mut self, method: &str, params: Value, ctx: &TurnContext) -> Result<(), AgentError> {
        self.guard(ctx)?;
        let result = self.io.send(
            &json!({"jsonrpc":"2.0","method":method,"params":params}),
            ctx,
        );
        if result.is_err() {
            self.abort();
        }
        result
    }
    fn request(
        &mut self,
        method: &str,
        params: Value,
        ctx: &TurnContext,
    ) -> Result<Value, AgentError> {
        self.guard(ctx)?;
        self.next += 1;
        let id = self.next;
        if id > 4096 {
            return Err(bad("LSP request budget exceeded"));
        }
        let result = (|| {
            self.io.send(
                &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
                ctx,
            )?;
            loop {
                let message = self.io.receive(ctx)?;
                self.received += 1;
                if self.received > 4096 || message["jsonrpc"] != "2.0" || !message.is_object() {
                    return Err(bad("invalid LSP message or message budget"));
                }
                if message.get("method").is_some() {
                    self.incoming(message, ctx)?;
                    continue;
                }
                if message["id"] != json!(id)
                    || message.get("result").is_some() == message.get("error").is_some()
                {
                    return Err(bad("unexpected LSP response ID or shape"));
                }
                if message.get("error").is_some() {
                    return Err(AgentError::Provider(
                        "LSP request rejected; server error body omitted".into(),
                    ));
                }
                return Ok(message["result"].clone());
            }
        })();
        if result.is_err() {
            self.abort();
        }
        result
    }
    fn incoming(&mut self, message: Value, ctx: &TurnContext) -> Result<(), AgentError> {
        if message.get("result").is_some() || message.get("error").is_some() {
            return Err(bad("invalid LSP request/notification shape"));
        }
        let method = message["method"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 128)
            .ok_or(bad("invalid LSP method"))?;
        if let Some(id) = message.get("id") {
            if !id.is_i64() && !id.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 128) {
                return Err(bad("invalid LSP server request ID"));
            }
            let result = match method {
                "workspace/configuration" => {
                    let items = message["params"]["items"]
                        .as_array()
                        .filter(|v| v.len() <= 64)
                        .ok_or(bad("invalid LSP configuration request"))?;
                    json!({"jsonrpc":"2.0","id":id,"result":vec![Value::Null;items.len()]})
                }
                "window/workDoneProgress/create" => json!({"jsonrpc":"2.0","id":id,"result":null}),
                "workspace/applyEdit" => {
                    json!({"jsonrpc":"2.0","id":id,"result":{"applied":false,"failureReason":"automatic server edits are disabled"}})
                }
                _ => {
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"unsupported client method"}})
                }
            };
            self.io.send(&result, ctx)?;
        } else if method == "textDocument/publishDiagnostics" {
            let p = &message["params"];
            let uri = p["uri"].as_str().ok_or(bad("invalid diagnostics URI"))?;
            self.workspace.relative(uri)?;
            if let Some(doc) = self.documents.get(uri) {
                let version = p.get("version");
                if version.is_some_and(|v| v != &json!(doc.version))
                    || version.is_none() && doc.version > 1
                {
                    return Ok(());
                }
                let items = self.parse_diagnostics(&p["diagnostics"], doc)?;
                self.diagnostics.insert(uri.into(), items);
            }
        }
        Ok(())
    }
    fn parse_diagnostics(
        &self,
        value: &Value,
        doc: &Document,
    ) -> Result<Vec<Diagnostic>, AgentError> {
        let array = value
            .as_array()
            .filter(|a| a.len() <= 512)
            .ok_or(bad("LSP diagnostic count limit"))?;
        let mut items = Vec::new();
        for value in array {
            let d: Diagnostic =
                serde_json::from_value(value.clone()).map_err(|_| bad("invalid LSP diagnostic"))?;
            if d.message.len() > 4096
                || d.source.as_ref().is_some_and(|s| s.len() > 256)
                || d.severity.is_some_and(|s| !(1..=4).contains(&s))
                || d.code
                    .as_ref()
                    .is_some_and(|c| !c.is_i64() && !c.as_str().is_some_and(|s| s.len() <= 256))
            {
                return Err(bad("LSP diagnostic field limit"));
            }
            if offset(&doc.text, d.range.start)? > offset(&doc.text, d.range.end)? {
                return Err(bad("reversed diagnostic range"));
            }
            items.push(d);
        }
        Ok(items)
    }
    pub fn open(
        &mut self,
        path: &str,
        language: &str,
        ctx: &TurnContext,
    ) -> Result<Document, AgentError> {
        self.guard(ctx)?;
        if self.documents.len() >= 32
            || language.is_empty()
            || language.len() > 64
            || !language
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_+-".contains(&b))
        {
            return Err(bad("invalid LSP document language or open limit"));
        }
        let sync = &self.capabilities["textDocumentSync"];
        if !(sync == 1 || sync == 2 || sync.is_object() && sync["openClose"] != false) {
            return Err(bad("LSP server does not support document synchronization"));
        }
        let uri = self.workspace.uri(path)?;
        if self.documents.contains_key(&uri) {
            return Err(bad("LSP document already open"));
        }
        let text = self.workspace.read(path, ctx)?;
        let doc = Document {
            uri: uri.clone(),
            version: 1,
            sha256: digest(&text),
            text,
            language: language.into(),
        };
        self.documents.insert(uri.clone(), doc.clone());
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument":{"uri":uri,"languageId":language,"version":1,"text":doc.text}}),
            ctx,
        )?;
        Ok(doc)
    }
    fn document(&self, uri: &str) -> Result<Document, AgentError> {
        self.documents
            .get(uri)
            .cloned()
            .ok_or(bad("LSP document not open"))
    }
    pub fn diagnostics(
        &mut self,
        uri: &str,
        ctx: &TurnContext,
    ) -> Result<Vec<Diagnostic>, AgentError> {
        self.guard(ctx)?;
        let doc = self.document(uri)?;
        if self
            .capabilities
            .get("diagnosticProvider")
            .is_some_and(|v| v == &Value::Bool(true) || v.is_object())
        {
            let report = self.request(
                "textDocument/diagnostic",
                json!({"textDocument":{"uri":uri}}),
                ctx,
            )?;
            if report["kind"] != "full" {
                return Err(bad("full LSP diagnostic report required"));
            }
            return self.parse_diagnostics(&report["items"], &doc);
        }
        let result = (|| {
            loop {
                self.guard(ctx)?;
                if let Some(items) = self.diagnostics.get(uri) {
                    return Ok(items.clone());
                }
                let message = self.io.receive(ctx)?;
                self.received += 1;
                if self.received > 4096
                    || message["jsonrpc"] != "2.0"
                    || message.get("method").is_none()
                {
                    return Err(bad("unexpected message while waiting for diagnostics"));
                }
                self.incoming(message, ctx)?;
            }
        })();
        if result.is_err() {
            self.abort();
        }
        result
    }
    fn feature(&self, name: &str) -> Result<(), AgentError> {
        if self
            .capabilities
            .get(name)
            .is_some_and(|v| v == &Value::Bool(true) || v.is_object())
        {
            Ok(())
        } else {
            Err(bad("LSP server capability unavailable"))
        }
    }
    fn locations(
        &mut self,
        uri: &str,
        position: Position,
        reference: bool,
        ctx: &TurnContext,
    ) -> Result<Vec<Location>, AgentError> {
        self.guard(ctx)?;
        self.feature(if reference {
            "referencesProvider"
        } else {
            "definitionProvider"
        })?;
        let doc = self.document(uri)?;
        offset(&doc.text, position)?;
        let mut params = json!({"textDocument":{"uri":uri},"position":position});
        if reference {
            params["context"] = json!({"includeDeclaration":true});
        }
        let result = self.request(
            if reference {
                "textDocument/references"
            } else {
                "textDocument/definition"
            },
            params,
            ctx,
        )?;
        let values = match result {
            Value::Null => vec![],
            Value::Array(a) => a,
            value => vec![value],
        };
        if values.len() > 256 {
            return Err(bad("LSP location count limit"));
        }
        let mut output = Vec::new();
        for value in values {
            let (uri, range) = if value.get("targetUri").is_some() {
                let range: Range = serde_json::from_value(value["targetSelectionRange"].clone())
                    .map_err(|_| bad("invalid LSP location link"))?;
                let target: Range = serde_json::from_value(value["targetRange"].clone())
                    .map_err(|_| bad("invalid LSP target range"))?;
                if target.start > range.start || target.end < range.end {
                    return Err(bad("selection outside LSP target range"));
                }
                (
                    value["targetUri"]
                        .as_str()
                        .ok_or(bad("invalid target URI"))?
                        .to_string(),
                    range,
                )
            } else {
                let location: Location =
                    serde_json::from_value(value).map_err(|_| bad("invalid LSP location"))?;
                (location.uri, location.range)
            };
            let path = self.workspace.relative(&uri)?;
            let text = if let Some(d) = self.documents.get(&uri) {
                d.text.clone()
            } else {
                self.workspace.read(
                    path.to_str().ok_or(bad("UTF-8 location path required"))?,
                    ctx,
                )?
            };
            if offset(&text, range.start)? > offset(&text, range.end)? {
                return Err(bad("reversed location range"));
            }
            output.push(Location { uri, range });
        }
        Ok(output)
    }
    pub fn definition(
        &mut self,
        uri: &str,
        position: Position,
        ctx: &TurnContext,
    ) -> Result<Vec<Location>, AgentError> {
        self.locations(uri, position, false, ctx)
    }
    pub fn references(
        &mut self,
        uri: &str,
        position: Position,
        ctx: &TurnContext,
    ) -> Result<Vec<Location>, AgentError> {
        self.locations(uri, position, true, ctx)
    }
    pub fn rename(
        &mut self,
        uri: &str,
        position: Position,
        new_name: &str,
        ctx: &TurnContext,
    ) -> Result<EditPlan, AgentError> {
        self.guard(ctx)?;
        self.feature("renameProvider")?;
        if new_name.is_empty() || new_name.len() > 256 || new_name.chars().any(|c| c.is_control()) {
            return Err(bad("invalid bounded LSP rename name"));
        }
        let doc = self.document(uri)?;
        offset(&doc.text, position)?;
        let result = self.request(
            "textDocument/rename",
            json!({"textDocument":{"uri":uri},"position":position,"newName":new_name}),
            ctx,
        )?;
        plan_from_edit(&doc, &result)
    }
    pub fn apply(&mut self, plan: &EditPlan, ctx: &TurnContext) -> Result<Applied, AgentError> {
        self.guard(ctx)?;
        let doc = self.document(&plan.uri)?;
        if doc.version != plan.version || doc.sha256 != plan.expected_sha256 {
            return Err(bad("stale LSP edit version or hash"));
        }
        let sync = &self.capabilities["textDocumentSync"];
        let change = if sync.is_object() {
            &sync["change"]
        } else {
            sync
        };
        if change != 1 && change != 2 {
            return Err(bad("LSP server does not accept text changes"));
        }
        let version = doc
            .version
            .checked_add(1)
            .ok_or(bad("LSP document version exhausted"))?;
        let text = self.workspace.preview(plan, ctx)?;
        // Admission of the complete didChange frame precedes the filesystem effect.
        let params = json!({"textDocument":{"uri":plan.uri,"version":version},"contentChanges":[{"text":text}]});
        if serde_json::to_vec(&params)
            .map_err(|_| bad("cannot encode didChange"))?
            .len()
            + 128
            > crate::transport::FRAME_BYTES
        {
            return Err(bad("LSP didChange frame limit"));
        }
        let applied = self.workspace.apply(plan, ctx)?;
        let mut updated = doc;
        updated.text = text;
        updated.sha256 = applied.sha256.clone();
        updated.version = version;
        self.documents.insert(plan.uri.clone(), updated);
        self.diagnostics.remove(&plan.uri);
        if self.notify("textDocument/didChange", params, ctx).is_err() {
            return Err(AgentError::State(
                "LSP edit was written but didChange failed; inspect file; do not replay",
            ));
        }
        Ok(applied)
    }
    pub fn close_document(&mut self, uri: &str, ctx: &TurnContext) -> Result<(), AgentError> {
        self.document(uri)?;
        self.notify(
            "textDocument/didClose",
            json!({"textDocument":{"uri":uri}}),
            ctx,
        )?;
        self.documents.remove(uri);
        self.diagnostics.remove(uri);
        Ok(())
    }
    pub fn close(&mut self, ctx: &TurnContext) -> CloseReport {
        if !self.closed && ctx.remaining().is_ok() {
            self.next += 1;
            let id = self.next;
            let result = (|| {
                self.io.send(
                    &json!({"jsonrpc":"2.0","id":id,"method":"shutdown","params":null}),
                    ctx,
                )?;
                loop {
                    let message = self.io.receive(ctx)?;
                    if message["jsonrpc"] != "2.0" {
                        return Err(bad("invalid LSP shutdown response"));
                    }
                    if message.get("method").is_some() {
                        self.received += 1;
                        if self.received > 4096 {
                            return Err(bad("LSP shutdown message limit"));
                        }
                        self.incoming(message, ctx)?;
                        continue;
                    }
                    if message["id"] != id
                        || message.get("error").is_some()
                        || message.get("result") != Some(&Value::Null)
                    {
                        return Err(bad("invalid LSP shutdown acknowledgement"));
                    }
                    break;
                }
                self.io.send(&json!({"jsonrpc":"2.0","method":"exit"}), ctx)
            })();
            if result.is_err() {
                self.closed = true;
                return self.io.stop(Duration::ZERO);
            }
        }
        self.closed = true;
        self.io.stop(Duration::from_millis(100))
    }
    fn abort(&mut self) {
        self.closed = true;
        self.io.stop(Duration::ZERO);
    }
}
pub fn plan_from_edit(doc: &Document, value: &Value) -> Result<EditPlan, AgentError> {
    let object = value
        .as_object()
        .ok_or(bad("LSP rename did not return WorkspaceEdit"))?;
    if object
        .keys()
        .any(|k| !matches!(k.as_str(), "changes" | "documentChanges"))
    {
        return Err(bad("unsupported WorkspaceEdit fields"));
    }
    let edits = match (value.get("changes"), value.get("documentChanges")) {
        (Some(changes), None) => {
            let map = changes
                .as_object()
                .filter(|m| m.len() == 1 && m.contains_key(&doc.uri))
                .ok_or(bad("only source-document edits are supported"))?;
            map[&doc.uri].clone()
        }
        (None, Some(changes)) => {
            let array = changes
                .as_array()
                .filter(|a| a.len() == 1)
                .ok_or(bad("single-document edits required"))?;
            let change = &array[0];
            if change.get("kind").is_some()
                || change["textDocument"]["uri"] != doc.uri
                || change["textDocument"]
                    .get("version")
                    .is_some_and(|v| !v.is_null() && v != &json!(doc.version))
            {
                return Err(bad("resource operation or stale document edit"));
            }
            change["edits"].clone()
        }
        _ => {
            return Err(bad(
                "exactly one WorkspaceEdit change representation required",
            ));
        }
    };
    let edits: Vec<TextEdit> =
        serde_json::from_value(edits).map_err(|_| bad("invalid LSP text edits"))?;
    crate::apply_text(&doc.text, &edits)?;
    Ok(EditPlan {
        uri: doc.uri.clone(),
        version: doc.version,
        expected_sha256: doc.sha256.clone(),
        edits,
    })
}
