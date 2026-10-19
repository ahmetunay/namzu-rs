//! Optional speech SDK: bounded immutable WAVs, explicit drivers and redacted lifecycle events.
mod wave;
mod workspace;
use namzu_core::{AgentError, TurnContext};
use serde::{Deserialize, Serialize};
pub use wave::{AudioInfo, MAX_AUDIO_BYTES, MAX_SECONDS, Wav};
pub use workspace::AudioWorkspace;
pub const MAX_TEXT_BYTES: usize = 16 * 1024;
pub const MAX_TRANSCRIPT_BYTES: usize = 64 * 1024;
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Transcript {
    pub text: String,
    pub language: Option<String>,
}
impl std::fmt::Debug for Transcript {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transcript")
            .field("text_bytes", &self.text.len())
            .finish()
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Transcribe,
    Synthesize,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AudioEvent {
    Started {
        operation: Operation,
    },
    InputValidated {
        info: AudioInfo,
    },
    Completed {
        operation: Operation,
        audio: Option<AudioInfo>,
        text_bytes: usize,
    },
    Failed {
        operation: Operation,
    },
}
pub trait AudioEvents {
    fn emit(&mut self, event: AudioEvent) -> Result<(), AgentError>;
}
impl AudioEvents for Vec<AudioEvent> {
    fn emit(&mut self, event: AudioEvent) -> Result<(), AgentError> {
        self.push(event);
        Ok(())
    }
}
pub trait SpeechInput {
    fn transcribe(
        &mut self,
        audio: &Wav,
        language: Option<&str>,
        ctx: &TurnContext,
    ) -> Result<Transcript, AgentError>;
}
pub trait SpeechOutput {
    fn synthesize(&mut self, text: &str, voice: &str, ctx: &TurnContext)
    -> Result<Wav, AgentError>;
}
pub fn validate_language(language: Option<&str>) -> Result<(), AgentError> {
    if language.is_some_and(|s| {
        s.len() < 2
            || s.len() > 16
            || !s
                .split('-')
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphabetic()))
    }) {
        return Err(AgentError::Configuration("invalid audio language tag"));
    }
    Ok(())
}
pub fn validate_speech(text: &str, voice: &str) -> Result<(), AgentError> {
    if text.trim().is_empty()
        || text.len() > MAX_TEXT_BYTES
        || text.chars().count() > 4000
        || text.contains('\0')
        || voice.is_empty()
        || voice.len() > 64
        || !voice
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
    {
        return Err(AgentError::Configuration("invalid speech text or voice"));
    }
    Ok(())
}
pub fn validate_transcript(value: &Transcript) -> Result<(), AgentError> {
    if value.text.len() > MAX_TRANSCRIPT_BYTES || value.text.contains('\0') {
        return Err(AgentError::Protocol("invalid transcript text"));
    }
    validate_language(value.language.as_deref())
        .map_err(|_| AgentError::Protocol("invalid transcript language"))
}
pub fn transcribe(
    driver: &mut impl SpeechInput,
    audio: &Wav,
    language: Option<&str>,
    ctx: &TurnContext,
    events: &mut impl AudioEvents,
) -> Result<Transcript, AgentError> {
    ctx.remaining()?;
    validate_language(language)?;
    events.emit(AudioEvent::Started {
        operation: Operation::Transcribe,
    })?;
    let result = (|| {
        events.emit(AudioEvent::InputValidated {
            info: audio.info().clone(),
        })?;
        ctx.remaining()?;
        let value = driver.transcribe(audio, language, ctx)?;
        ctx.remaining()?;
        validate_transcript(&value)?;
        events.emit(AudioEvent::Completed {
            operation: Operation::Transcribe,
            audio: None,
            text_bytes: value.text.len(),
        })?;
        Ok(value)
    })();
    if result.is_err() {
        let _ = events.emit(AudioEvent::Failed {
            operation: Operation::Transcribe,
        });
    }
    result
}
pub fn synthesize(
    driver: &mut impl SpeechOutput,
    text: &str,
    voice: &str,
    ctx: &TurnContext,
    events: &mut impl AudioEvents,
) -> Result<Wav, AgentError> {
    ctx.remaining()?;
    validate_speech(text, voice)?;
    events.emit(AudioEvent::Started {
        operation: Operation::Synthesize,
    })?;
    let result = (|| {
        ctx.remaining()?;
        let audio = driver.synthesize(text, voice, ctx)?;
        ctx.remaining()?;
        events.emit(AudioEvent::Completed {
            operation: Operation::Synthesize,
            audio: Some(audio.info().clone()),
            text_bytes: text.len(),
        })?;
        Ok(audio)
    })();
    if result.is_err() {
        let _ = events.emit(AudioEvent::Failed {
            operation: Operation::Synthesize,
        });
    }
    result
}
