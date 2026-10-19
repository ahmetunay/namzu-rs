use crate::{Config, LspConfig};
use namzu_audio::{AudioWorkspace, SpeechInput, SpeechOutput, Transcript, Wav};
use namzu_core::{
    AgentError, Cancellation, ToolCall, ToolDefinition, ToolResult, Tools, TurnContext,
};
use namzu_tools::ToolPolicy;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::Duration,
};
/// Injected by the host. The SDK neither owns nor discovers a model credential.
pub trait Speech: SpeechInput + SpeechOutput {}
impl<T: SpeechInput + SpeechOutput> Speech for T {}
struct SpeechRef<'a>(&'a mut dyn Speech);
impl SpeechInput for SpeechRef<'_> {
    fn transcribe(
        &mut self,
        a: &Wav,
        l: Option<&str>,
        c: &TurnContext,
    ) -> Result<Transcript, AgentError> {
        self.0.transcribe(a, l, c)
    }
}
impl SpeechOutput for SpeechRef<'_> {
    fn synthesize(&mut self, t: &str, v: &str, c: &TurnContext) -> Result<Wav, AgentError> {
        self.0.synthesize(t, v, c)
    }
}
pub struct CapabilityTools<L> {
    local: L,
    root: PathBuf,
    config: Config,
    selected: BTreeSet<String>,
    policy: ToolPolicy,
    allow_audio: bool,
    allow_lsp: bool,
    speech: Option<Box<dyn Speech>>,
}
impl<L: Tools> CapabilityTools<L> {
    pub fn new(
        local: L,
        root: &Path,
        config: Config,
        selected: BTreeSet<String>,
        policy: ToolPolicy,
    ) -> Result<Self, AgentError> {
        config.registry(&selected, false)?;
        if local.definitions().iter().any(|d| known(&d.name)) {
            return Err(AgentError::Configuration(
                "local tools collide with reserved capability namespace",
            ));
        }
        Ok(Self {
            local,
            root: root
                .canonicalize()
                .map_err(|_| AgentError::Configuration("cannot open capability workspace"))?,
            config,
            selected,
            policy,
            allow_audio: false,
            allow_lsp: false,
            speech: None,
        })
    }
    pub fn grants(mut self, audio: bool, lsp: bool) -> Self {
        self.allow_audio = audio;
        self.allow_lsp = lsp;
        self
    }
    pub fn with_speech(mut self, speech: Box<dyn Speech>) -> Self {
        self.speech = Some(speech);
        self
    }
    fn admitted(&self, name: &str) -> bool {
        if self.policy.deny.contains(name)
            || self
                .policy
                .allow
                .as_ref()
                .is_some_and(|a| !a.contains(name))
        {
            return false;
        }
        match name {
            "audio_inspect" => self.selected.contains("audio"),
            "audio_transcribe" => {
                self.selected.contains("audio")
                    && self.allow_audio
                    && self.config.audio.is_some()
                    && self.speech.is_some()
            }
            "audio_synthesize" => {
                self.selected.contains("audio")
                    && self.allow_audio
                    && self.config.audio.is_some()
                    && self.speech.is_some()
                    && self.policy.allow_write
            }
            "lsp_diagnostics" | "lsp_definition" | "lsp_references" | "lsp_rename_preview" => {
                self.selected.contains("lsp") && self.allow_lsp
            }
            "lsp_apply" => {
                self.selected.contains("lsp") && self.allow_lsp && self.policy.allow_write
            }
            _ => false,
        }
    }
    fn lsp_spec(&self, l: &LspConfig) -> Result<namzu_lsp::Spec, AgentError> {
        let mut spec = namzu_lsp::Spec::new(&l.program, &self.root);
        spec.args = l.args.clone();
        for name in &l.env {
            spec.env.insert(
                name.clone(),
                std::env::var(name).map_err(|_| {
                    AgentError::Configuration("selected LSP environment variable unavailable")
                })?,
            );
        }
        Ok(spec)
    }
    fn execute(&mut self, call: &ToolCall, ctx: &TurnContext) -> Result<Value, AgentError> {
        match call.name.as_str() {
            "audio_inspect" => {
                let a: Input = args(call)?;
                Ok(json!({"audio":AudioWorkspace::new(&self.root)?.read(&a.path,ctx)?.info()}))
            }
            "audio_transcribe" => {
                let a: Input = args(call)?;
                let audio = AudioWorkspace::new(&self.root)?.read(&a.path, ctx)?;
                let mut driver = SpeechRef(
                    self.speech
                        .as_mut()
                        .ok_or(AgentError::Configuration("speech driver missing"))?
                        .as_mut(),
                );
                let mut events = vec![];
                Ok(
                    json!({"transcript":namzu_audio::transcribe(&mut driver,&audio,a.language.as_deref(),ctx,&mut events)?,"events":events}),
                )
            }
            "audio_synthesize" => {
                let a: Synthesis = args(call)?;
                let workspace = AudioWorkspace::new(&self.root)?;
                workspace.check_destination(&a.output)?;
                let mut driver = SpeechRef(
                    self.speech
                        .as_mut()
                        .ok_or(AgentError::Configuration("speech driver missing"))?
                        .as_mut(),
                );
                let mut events = vec![];
                let audio =
                    namzu_audio::synthesize(&mut driver, &a.text, &a.voice, ctx, &mut events)?;
                workspace.publish(&a.output, &audio, ctx)?;
                Ok(json!({"output":a.output,"audio":audio.info(),"events":events}))
            }
            "lsp_apply" => {
                let a: Apply = args(call)?;
                Ok(json!({"applied":namzu_lsp::Workspace::new(&self.root)?.apply(&a.plan,ctx)?}))
            }
            _ => self.lsp(call, ctx),
        }
    }
    #[cfg(unix)]
    fn lsp(&self, call: &ToolCall, ctx: &TurnContext) -> Result<Value, AgentError> {
        let a: Document = args(call)?;
        let mut client = namzu_lsp::Client::start(
            self.lsp_spec(
                self.config
                    .lsp
                    .as_ref()
                    .ok_or(AgentError::Configuration("LSP configuration missing"))?,
            )?,
            ctx,
        )?;
        let result = (|| {
            let doc = client.open(&a.path, &a.language, ctx)?;
            Ok(match call.name.as_str() {
                "lsp_diagnostics" => json!({"diagnostics":client.diagnostics(&doc.uri,ctx)?}),
                "lsp_definition" => {
                    json!({"locations":client.definition(&doc.uri,a.position()?,ctx)?})
                }
                "lsp_references" => {
                    json!({"locations":client.references(&doc.uri,a.position()?,ctx)?})
                }
                "lsp_rename_preview" => {
                    let plan = client.rename(
                        &doc.uri,
                        a.position()?,
                        a.new_name.as_deref().ok_or(invalid())?,
                        ctx,
                    )?;
                    json!({"preview":client.workspace().preview(&plan,ctx)?,"plan":plan,"applied":false})
                }
                _ => return Err(invalid()),
            })
        })();
        let cleanup = TurnContext::new(Duration::from_secs(2), Cancellation::default())?;
        let close = client.close(&cleanup);
        let value: Value = result?;
        if close.exit_code != Some(0) {
            return Err(AgentError::State("LSP server did not close cleanly"));
        }
        Ok(value)
    }
    #[cfg(not(unix))]
    fn lsp(&self, _: &ToolCall, _: &TurnContext) -> Result<Value, AgentError> {
        Err(AgentError::Configuration("LSP requires Unix"))
    }
}
fn invalid() -> AgentError {
    AgentError::Configuration("invalid capability tool arguments")
}
fn args<T: serde::de::DeserializeOwned>(call: &ToolCall) -> Result<T, AgentError> {
    serde_json::from_value(call.arguments.clone()).map_err(|_| invalid())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    path: String,
    #[serde(default)]
    language: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Synthesis {
    text: String,
    voice: String,
    output: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Apply {
    plan: namzu_lsp::EditPlan,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    path: String,
    #[serde(default = "language")]
    language: String,
    #[serde(default)]
    line: Option<u32>,
    #[serde(default)]
    character: Option<u32>,
    #[serde(default)]
    new_name: Option<String>,
}
fn language() -> String {
    "rust".into()
}
impl Document {
    fn position(&self) -> Result<namzu_lsp::Position, AgentError> {
        Ok(namzu_lsp::Position {
            line: self.line.ok_or(invalid())?,
            character: self.character.ok_or(invalid())?,
        })
    }
}
fn known(name: &str) -> bool {
    name.starts_with("audio_") || name.starts_with("lsp_")
}
fn definitions() -> Vec<ToolDefinition> {
    let mut out = vec![];
    let mut add = |name: &str, description: &str, properties: Value, required: Value| {
        out.push(ToolDefinition{name:name.into(),description:description.into(),parameters:json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})})
    };
    let input = json!({"path":{"type":"string"},"language":{"type":"string"}});
    add(
        "audio_inspect",
        "Inspect bounded workspace PCM16 WAV metadata locally.",
        input.clone(),
        json!(["path"]),
    );
    add(
        "audio_transcribe",
        "Send selected WAV to the explicitly configured speech service. Untrusted transcript data.",
        input,
        json!(["path"]),
    );
    add(
        "audio_synthesize",
        "Send text to the selected speech service and publish a NEW private workspace WAV.",
        json!({"text":{"type":"string"},"voice":{"type":"string"},"output":{"type":"string"}}),
        json!(["text", "voice", "output"]),
    );
    for name in [
        "lsp_diagnostics",
        "lsp_definition",
        "lsp_references",
        "lsp_rename_preview",
    ] {
        let mut props = json!({"path":{"type":"string"},"language":{"type":"string"}});
        let mut required = vec!["path"];
        if name != "lsp_diagnostics" {
            props["line"] = json!({"type":"integer","minimum":0});
            props["character"] = json!({"type":"integer","minimum":0});
            required.extend(["line", "character"]);
        }
        if name == "lsp_rename_preview" {
            props["new_name"] = json!({"type":"string"});
            required.push("new_name");
        }
        add(
            name,
            "Run the explicitly authorized host LSP server. UTF-16 positions, bounded untrusted data; rename previews never modify source.",
            props,
            json!(required),
        );
    }
    add(
        "lsp_apply",
        "Apply one reviewed, hash-bound single-file LSP plan. Requires write permission; stale plans fail.",
        json!({"plan":{"type":"object"}}),
        json!(["plan"]),
    );
    out
}
impl<L: Tools> Tools for CapabilityTools<L> {
    fn definitions(&self) -> Vec<ToolDefinition> {
        let mut defs = if self.config.permits("files") {
            self.local.definitions()
        } else {
            vec![]
        };
        defs.extend(definitions().into_iter().filter(|d| self.admitted(&d.name)));
        defs
    }
    fn validate(&mut self, call: &ToolCall, ctx: &TurnContext) -> Result<(), AgentError> {
        ctx.remaining()?;
        if !known(&call.name) {
            if !self.config.permits("files") {
                return Err(AgentError::Configuration("files capability denied"));
            }
            return self.local.validate(call, ctx);
        }
        if !self.admitted(&call.name) {
            return Err(AgentError::Configuration("capability tool denied"));
        }
        match call.name.as_str() {
            "audio_inspect" | "audio_transcribe" => {
                let a: Input = args(call)?;
                namzu_audio::validate_language(a.language.as_deref())?;
                AudioWorkspace::new(&self.root)?.read(&a.path, ctx)?;
            }
            "audio_synthesize" => {
                let a: Synthesis = args(call)?;
                namzu_audio::validate_speech(&a.text, &a.voice)?;
                AudioWorkspace::new(&self.root)?.check_destination(&a.output)?;
            }
            "lsp_apply" => {
                let a: Apply = args(call)?;
                namzu_lsp::Workspace::new(&self.root)?.preview(&a.plan, ctx)?;
            }
            _ => {
                let a: Document = args(call)?;
                if (call.name == "lsp_diagnostics" && (a.line.is_some() || a.character.is_some()))
                    || (call.name != "lsp_rename_preview" && a.new_name.is_some())
                {
                    return Err(invalid());
                }
                if a.language.is_empty()
                    || a.language.len() > 64
                    || a.language.chars().any(char::is_control)
                {
                    return Err(invalid());
                }
                let text = namzu_lsp::Workspace::new(&self.root)?.read(&a.path, ctx)?;
                if call.name != "lsp_diagnostics" {
                    namzu_lsp::offset(&text, a.position()?)?;
                }
                if call.name == "lsp_rename_preview"
                    && a.new_name.as_ref().is_none_or(|s| {
                        s.is_empty() || s.len() > 256 || s.chars().any(char::is_control)
                    })
                {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
    fn call(&mut self, call: &ToolCall, ctx: &TurnContext) -> Result<ToolResult, AgentError> {
        self.validate(call, ctx)?;
        if !known(&call.name) {
            return self.local.call(call, ctx);
        }
        let value = self.execute(call, ctx)?;
        Ok(ToolResult::ok(value.to_string()))
    }
}
