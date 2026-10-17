use super::{Endpoints, client, http_error, read_json};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use namzu_core::{AgentError, TurnContext};
use namzu_session::{PrivateStore, now_ms};
use reqwest::{blocking::Client, header::HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::Path,
    time::{Duration, Instant},
};

pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const FILE: &str = "codex-credentials.json";

// Deliberately no Debug: tokens must not appear in diagnostics.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credential {
    version: u32,
    pub(super) access_token: String,
    refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id_token: Option<String>,
    pub(super) account_id: String,
    expires_ms: u64,
}
impl Credential {
    fn validate(&self) -> Result<(), AgentError> {
        if self.version != 1
            || self.access_token.is_empty()
            || self.access_token.len() > 65536
            || self.account_id.is_empty()
            || self.account_id.len() > 256
            || self
                .refresh_token
                .as_ref()
                .is_some_and(|value| value.is_empty() || value.len() > 65536)
            || self
                .id_token
                .as_ref()
                .is_some_and(|value| value.is_empty() || value.len() > 65536)
            || self.expires_ms == 0
        {
            return Err(AgentError::State("invalid Codex credential"));
        }
        HeaderValue::from_str(&format!("Bearer {}", self.access_token))
            .map_err(|_| AgentError::State("invalid Codex authorization header"))?;
        HeaderValue::from_str(&self.account_id)
            .map_err(|_| AgentError::State("invalid Codex account header"))?;
        Ok(())
    }
}

fn token_credential(body: &Value, previous: Option<&Credential>) -> Result<Credential, AgentError> {
    let token = body["access_token"]
        .as_str()
        .filter(|token| !token.is_empty() && token.len() <= 65536)
        .ok_or(AgentError::Protocol("missing access token"))?;
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(AgentError::Protocol("expected Codex access token claims"));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|_| AgentError::Protocol("invalid access token metadata"))?;
    let claims: Value = serde_json::from_slice(&bytes)
        .map_err(|_| AgentError::Protocol("invalid access token metadata"))?;
    let account = claims["https://api.openai.com/auth"]["chatgpt_account_id"]
        .as_str()
        .ok_or(AgentError::Protocol(
            "token did not identify a ChatGPT account",
        ))?;
    if previous.is_some_and(|old| old.account_id != account) {
        return Err(AgentError::Protocol("refreshed account identity changed"));
    }
    let expires_ms = claims["exp"]
        .as_u64()
        .and_then(|value| value.checked_mul(1000))
        .ok_or(AgentError::Protocol("token has no bounded expiry"))?;
    if expires_ms <= now_ms() {
        return Err(AgentError::Protocol(
            "service returned expired access token",
        ));
    }
    let refresh = body
        .get("refresh_token")
        .map(|value| {
            value
                .as_str()
                .filter(|s| !s.is_empty())
                .map(String::from)
                .ok_or(AgentError::Protocol("invalid refresh grant"))
        })
        .transpose()?
        .or_else(|| previous.and_then(|old| old.refresh_token.clone()));
    let id_token = body
        .get("id_token")
        .map(|value| {
            value
                .as_str()
                .filter(|value| !value.is_empty() && value.len() <= 65536)
                .map(String::from)
                .ok_or(AgentError::Protocol("invalid identity token"))
        })
        .transpose()?
        .or_else(|| previous.and_then(|old| old.id_token.clone()));
    let credential = Credential {
        version: 1,
        access_token: token.into(),
        refresh_token: refresh,
        id_token,
        account_id: account.into(),
        expires_ms,
    };
    credential.validate()?;
    Ok(credential)
}

pub fn status(path: &Path) -> Result<Value, AgentError> {
    let store = PrivateStore::open(path)?;
    let credential: Option<Credential> = store.read(FILE)?;
    if let Some(credential) = credential {
        credential.validate()?;
        Ok(
            json!({"authenticated":true,"expired":credential.expires_ms<=now_ms(),"expires_ms":credential.expires_ms,"refresh_available":credential.refresh_token.is_some()}),
        )
    } else {
        Ok(json!({"authenticated":false}))
    }
}
pub fn logout(path: &Path) -> Result<(), AgentError> {
    let store = PrivateStore::open(path)?;
    let _lock = store.lock("codex-auth")?;
    store.remove(FILE)
}

/// Explicit one-time access/identity-token copy to a NEW private Jcode profile.
/// The refresh grant stays with Namzu, preventing cross-client rotation races.
pub fn link_jcode(path: &Path, directory: &Path, context: &TurnContext) -> Result<(), AgentError> {
    link_jcode_at(
        path,
        directory,
        &Endpoints::production(),
        &client()?,
        context,
    )
}
pub(super) fn link_jcode_at(
    path: &Path,
    directory: &Path,
    endpoints: &Endpoints,
    http: &Client,
    context: &TurnContext,
) -> Result<(), AgentError> {
    let target = PrivateStore::open(directory)?;
    let _lock = target.lock("namzu-jcode-link")?;
    if target.read::<Value>("openai-auth.json")?.is_some() {
        return Err(AgentError::State("Jcode credential store already exists"));
    }
    // Jcode identifies ChatGPT mode by a refresh grant OR an identity token.
    // Old Namzu stores omitted identity tokens; refresh once under our auth lock.
    let credential = ensure_fresh_for(path, endpoints, http, context, true)?;
    let id_token = credential.id_token.ok_or(AgentError::State(
        "identity token missing after refresh; run auth login before linking Jcode",
    ))?;
    context.remaining()?;
    target.write_new("openai-auth.json",&json!({"openai_accounts":[{"label":"openai-otter","access_token":credential.access_token,"id_token":id_token,"refresh_token":"","account_id":credential.account_id,"expires_at":credential.expires_ms}],"active_openai_account":"openai-otter"}))
}

pub(super) fn ensure_fresh(
    path: &Path,
    endpoints: &Endpoints,
    http: &Client,
    context: &TurnContext,
) -> Result<Credential, AgentError> {
    ensure_fresh_for(path, endpoints, http, context, false)
}
fn ensure_fresh_for(
    path: &Path,
    endpoints: &Endpoints,
    http: &Client,
    context: &TurnContext,
    require_identity: bool,
) -> Result<Credential, AgentError> {
    context.remaining()?;
    let store = PrivateStore::open(path)?;
    let _lock = store.lock("codex-auth")?;
    let current: Credential = store
        .read(FILE)?
        .ok_or(AgentError::State("Codex login required; run auth login"))?;
    current.validate()?;
    if current.expires_ms.saturating_sub(now_ms()) > 60_000
        && (!require_identity || current.id_token.is_some())
    {
        return Ok(current);
    }
    let refresh = current.refresh_token.as_ref().ok_or(AgentError::State(
        "Codex refresh grant missing; login required",
    ))?;
    let response = http
        .post(
            endpoints
                .auth
                .join("oauth/token")
                .map_err(|_| AgentError::Configuration("invalid auth endpoint"))?,
        )
        .timeout(context.remaining()?.min(Duration::from_secs(30)))
        .json(&json!({"grant_type":"refresh_token","refresh_token":refresh,"client_id":CLIENT_ID}))
        .send()
        .map_err(http_error)?;
    let body = read_json(response, context)?;
    let credential = token_credential(&body, Some(&current))?;
    context.remaining()?;
    store.write(FILE, &credential)?;
    Ok(credential)
}

pub fn login_device(
    path: &Path,
    context: &TurnContext,
    on_code: impl FnMut(&str, &str),
) -> Result<(), AgentError> {
    login_at(path, &Endpoints::production(), context, on_code)
}
pub(super) fn login_at(
    path: &Path,
    endpoints: &Endpoints,
    context: &TurnContext,
    on_code: impl FnMut(&str, &str),
) -> Result<(), AgentError> {
    let http = client()?;
    login_with_client(path, endpoints, &http, context, on_code)
}
pub(super) fn login_with_client(
    path: &Path,
    endpoints: &Endpoints,
    http: &Client,
    context: &TurnContext,
    mut on_code: impl FnMut(&str, &str),
) -> Result<(), AgentError> {
    let store = PrivateStore::open(path)?;
    let _lock = store.lock("codex-auth")?;
    let response = http
        .post(
            endpoints
                .auth
                .join("api/accounts/deviceauth/usercode")
                .unwrap(),
        )
        .timeout(context.remaining()?.min(Duration::from_secs(30)))
        .json(&json!({"client_id":CLIENT_ID}))
        .send()
        .map_err(http_error)?;
    let body = read_json(response, context)?;
    let device = body["device_auth_id"]
        .as_str()
        .filter(|value| !value.is_empty() && value.len() <= 4096)
        .ok_or(AgentError::Protocol("invalid device authorization ID"))?;
    let user_code = body
        .get("user_code")
        .or_else(|| body.get("usercode"))
        .and_then(Value::as_str)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 64
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .ok_or(AgentError::Protocol("invalid device user code"))?;
    let interval = body["interval"]
        .as_f64()
        .or_else(|| body["interval"].as_str().and_then(|s| s.parse().ok()))
        .filter(|value| value.is_finite() && *value >= 0.0 && *value <= 60.0)
        .ok_or(AgentError::Protocol("invalid device polling interval"))?;
    let interval = Duration::from_secs_f64(interval.max(0.01));
    let url = endpoints.auth.join("codex/device").unwrap();
    // The documented verification page is /codex/device, not a token endpoint.
    on_code(url.as_str(), user_code);
    loop {
        context.remaining()?;
        let response = http
            .post(
                endpoints
                    .auth
                    .join("api/accounts/deviceauth/token")
                    .unwrap(),
            )
            .timeout(context.remaining()?.min(Duration::from_secs(30)))
            .json(&json!({"device_auth_id":device,"user_code":user_code}))
            .send()
            .map_err(http_error)?;
        if response.status().is_success() {
            let authorization = read_json(response, context)?;
            let code = authorization["authorization_code"]
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 16384)
                .ok_or(AgentError::Protocol("missing authorization code"))?;
            let verifier = authorization["code_verifier"]
                .as_str()
                .filter(|s| {
                    (43..=128).contains(&s.len())
                        && s.bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
                })
                .ok_or(AgentError::Protocol("invalid authorization verifier"))?;
            let redirect = endpoints.auth.join("deviceauth/callback").unwrap();
            let response = http
                .post(endpoints.auth.join("oauth/token").unwrap())
                .timeout(context.remaining()?.min(Duration::from_secs(30)))
                .form(&[
                    ("grant_type", "authorization_code"),
                    ("code", code),
                    ("redirect_uri", redirect.as_str()),
                    ("client_id", CLIENT_ID),
                    ("code_verifier", verifier),
                ])
                .send()
                .map_err(http_error)?;
            let credential = token_credential(&read_json(response, context)?, None)?;
            context.remaining()?;
            return store.write(FILE, &credential);
        }
        if ![403, 404].contains(&response.status().as_u16()) {
            return Err(AgentError::Provider(format!(
                "device login HTTP status {}",
                response.status().as_u16()
            )));
        }
        drop(response);
        let next = Instant::now()
            .checked_add(interval)
            .ok_or(AgentError::Protocol("polling interval overflow"))?;
        while Instant::now() < next {
            std::thread::sleep(
                Duration::from_millis(50)
                    .min(next.saturating_duration_since(Instant::now()))
                    .min(context.remaining()?),
            );
        }
    }
}

#[cfg(test)]
pub(super) fn fixture_credential(account: &str, expiry: u64) -> Credential {
    let claims = json!({"exp":expiry,"https://api.openai.com/auth":{"chatgpt_account_id":account}});
    Credential {
        version: 1,
        access_token: format!(
            "fixture.{}.signature",
            URL_SAFE_NO_PAD.encode(claims.to_string())
        ),
        refresh_token: Some("fixture-refresh".into()),
        id_token: Some("fixture-identity-token".into()),
        account_id: account.into(),
        expires_ms: expiry * 1000,
    }
}
#[cfg(test)]
pub(super) fn save_fixture(path: &Path, credential: &Credential) {
    PrivateStore::open(path)
        .unwrap()
        .write(FILE, credential)
        .unwrap();
}
