//! Explicit OpenAI-compatible speech HTTP. No subscription-token discovery or automatic replay.
use crate::http::{self, Client};
use namzu_audio::{
    MAX_AUDIO_BYTES, SpeechInput, SpeechOutput, Transcript, Wav, validate_language,
    validate_speech, validate_transcript,
};
use namzu_core::{AgentError, TurnContext};
use reqwest::{
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue},
    redirect::Policy,
};
use serde_json::json;
use std::io::Read;
use url::Url;
pub struct SpeechClient {
    client: Client,
    transcriptions: Url,
    speech: Url,
    authorization: HeaderValue,
    input_model: String,
    output_model: String,
}
impl SpeechClient {
    pub fn new(
        base_url: &str,
        api_key: &str,
        input_model: impl Into<String>,
        output_model: impl Into<String>,
    ) -> Result<Self, AgentError> {
        let mut base = Url::parse(base_url)
            .map_err(|_| AgentError::Configuration("invalid speech base URL"))?;
        let loopback = match base.host() {
            Some(url::Host::Ipv4(v)) => v.is_loopback(),
            Some(url::Host::Ipv6(v)) => v.is_loopback(),
            _ => base.host_str() == Some("localhost"),
        };
        if base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || !(base.scheme() == "https" || base.scheme() == "http" && loopback)
        {
            return Err(AgentError::Configuration(
                "speech requires HTTPS, or loopback HTTP, without URL credentials/query/fragment",
            ));
        }
        let input_model = input_model.into();
        let output_model = output_model.into();
        if [&input_model, &output_model]
            .iter()
            .any(|m| m.is_empty() || m.len() > 256 || !m.bytes().all(|b| (33..=126).contains(&b)))
            || api_key.is_empty()
        {
            return Err(AgentError::Configuration(
                "speech models and explicit API key required",
            ));
        }
        let mut authorization = HeaderValue::from_str(&format!("Bearer {api_key}"))
            .map_err(|_| AgentError::Configuration("invalid speech key header"))?;
        authorization.set_sensitive(true);
        let path = base.path().trim_end_matches('/').to_string();
        base.set_path(&format!("{path}/audio/transcriptions"));
        let transcriptions = base.clone();
        base.set_path(&format!("{path}/audio/speech"));
        Ok(Self {
            client: Client::builder().redirect(Policy::none()).build()?,
            transcriptions,
            speech: base,
            authorization,
            input_model,
            output_model,
        })
    }
    fn bytes(
        &self,
        request: reqwest::RequestBuilder,
        limit: usize,
        ctx: &TurnContext,
    ) -> Result<Vec<u8>, AgentError> {
        let response = self.client.send(request.timeout(ctx.remaining()?), ctx)?;
        ctx.remaining()?;
        if !response.status().is_success() {
            return Err(AgentError::Provider(format!(
                "speech HTTP status {}; request not replayed",
                response.status().as_u16()
            )));
        }
        let mut bytes = Vec::new();
        response
            .take((limit + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| http::read_error(ctx))?;
        ctx.remaining()?;
        if bytes.len() > limit {
            return Err(AgentError::Protocol("speech response exceeds byte budget"));
        }
        Ok(bytes)
    }
}
impl SpeechInput for SpeechClient {
    fn transcribe(
        &mut self,
        audio: &Wav,
        language: Option<&str>,
        ctx: &TurnContext,
    ) -> Result<Transcript, AgentError> {
        ctx.remaining()?;
        validate_language(language)?;
        let boundary = format!("namzu-speech-{}", namzu_session::new_id());
        // Refuse delimiter collisions, including in untrusted binary samples/model fields.
        if audio
            .bytes()
            .windows(boundary.len())
            .any(|v| v == boundary.as_bytes())
            || self.input_model.contains(&boundary)
        {
            return Err(AgentError::Protocol("speech multipart boundary collision"));
        }
        let mut body = Vec::with_capacity(audio.bytes().len() + 2048);
        for (name, value) in [
            ("model", self.input_model.as_str()),
            ("response_format", "json"),
        ]
        .into_iter()
        .chain(language.map(|v| ("language", v)))
        {
            body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
        }
        body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"input.wav\"\r\nContent-Type: audio/wav\r\n\r\n").as_bytes());
        body.extend_from_slice(audio.bytes());
        body.extend(format!("\r\n--{boundary}--\r\n").as_bytes());
        let bytes = self.bytes(
            self.client
                .post(self.transcriptions.clone())
                .header(AUTHORIZATION, self.authorization.clone())
                .header(
                    CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(body),
            128 * 1024,
            ctx,
        )?;
        #[derive(serde::Deserialize)]
        struct Response {
            text: String,
            #[serde(default)]
            language: Option<String>,
        }
        let parsed: Response = serde_json::from_slice(&bytes)
            .map_err(|_| AgentError::Protocol("invalid speech transcript JSON"))?;
        let transcript = Transcript {
            text: parsed.text,
            language: parsed.language,
        };
        validate_transcript(&transcript)?;
        Ok(transcript)
    }
}
impl SpeechOutput for SpeechClient {
    fn synthesize(
        &mut self,
        text: &str,
        voice: &str,
        ctx: &TurnContext,
    ) -> Result<Wav, AgentError> {
        ctx.remaining()?;
        validate_speech(text, voice)?;
        let bytes=self.bytes(self.client.post(self.speech.clone()).header(AUTHORIZATION,self.authorization.clone()).json(&json!({"model":self.output_model,"input":text,"voice":voice,"response_format":"wav"})),MAX_AUDIO_BYTES,ctx)?;
        let wav = Wav::from_http_body(bytes)?;
        ctx.remaining()?;
        Ok(wav)
    }
}

#[cfg(test)]
mod tests;
