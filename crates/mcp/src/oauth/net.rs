use super::*;
use serde_json::Value;
use std::{future::Future, time::Duration};
pub(super) struct Net {
    http: reqwest::Client,
    runtime: tokio::runtime::Runtime,
}
async fn wait<T>(
    ctx: &TurnContext,
    f: impl Future<Output = Result<T, reqwest::Error>>,
) -> Result<T, Error> {
    let remaining = ctx.remaining()?;
    tokio::select! {biased;
        cause=async {loop {if let Err(e)=ctx.remaining(){return e} tokio::time::sleep(Duration::from_millis(10)).await;}}=>Err(Error::Context(cause)),
        result=tokio::time::timeout(remaining,f)=>{ctx.remaining()?;result.map_err(|_|Error::Context(AgentError::Timeout))?.map_err(|_|Error::Network)}
    }
}
impl Net {
    pub fn new() -> Result<Self, Error> {
        Ok(Self {
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .build()
                .map_err(|_| Error::Invalid)?,
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| Error::Invalid)?,
        })
    }
    fn send(
        &self,
        r: reqwest::RequestBuilder,
        ctx: &TurnContext,
    ) -> Result<reqwest::Response, Error> {
        self.runtime.block_on(wait(ctx, r.send()))
    }
    fn json(&self, mut r: reqwest::Response, ctx: &TurnContext) -> Result<Value, Error> {
        if !r.status().is_success() {
            return Err(Error::Status(r.status().as_u16()));
        }
        if !r
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .is_some_and(|s| {
                s.split(';')
                    .next()
                    .is_some_and(|s| s.trim().eq_ignore_ascii_case("application/json"))
            })
        {
            return Err(Error::Invalid);
        }
        if r.content_length().is_some_and(|n| n > 65536) {
            return Err(Error::Limit);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = self.runtime.block_on(wait(ctx, r.chunk()))? {
            if bytes.len() + chunk.len() > 65536 {
                return Err(Error::Limit);
            }
            bytes.extend_from_slice(&chunk);
        }
        ctx.remaining()?;
        let v: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
        if !v.is_object() {
            return Err(Error::Invalid);
        }
        Ok(v)
    }
    fn get(&self, url: Url, ctx: &TurnContext) -> Result<Value, Error> {
        let response = self.send(self.http.get(url).header("Accept", "application/json"), ctx)?;
        self.json(response, ctx)
    }
    pub fn discover(&self, t: &Target, ctx: &TurnContext) -> Result<Metadata, Error> {
        let resource = endpoint(&t.resource)?;
        let response = self.send(
            self.http
                .get(resource.clone())
                .header("Accept", "application/json"),
            ctx,
        )?;
        if ![200, 401, 403, 405].contains(&response.status().as_u16()) {
            return Err(Error::Status(response.status().as_u16()));
        }
        let mut advertised = None;
        for header in response
            .headers()
            .get_all(reqwest::header::WWW_AUTHENTICATE)
        {
            let (url, scopes) = challenge(header.to_str().map_err(|_| Error::Invalid)?)?;
            if let Some(scopes) = scopes
                && scopes
                    .split(' ')
                    .any(|s| !scope(s) || !t.settings.scopes.iter().any(|v| v == s))
            {
                return Err(Error::Invalid);
            }
            if let Some(url) = url
                && advertised.replace(url).is_some()
            {
                return Err(Error::Invalid);
            }
        }
        drop(response);
        let mut candidates = Vec::new();
        if let Some(url) = advertised {
            let url = endpoint(&url)?;
            if url.origin() != resource.origin() {
                return Err(Error::Invalid);
            }
            candidates.push(url);
        } else {
            let mut path = resource.clone();
            path.set_path(&format!(
                "/.well-known/oauth-protected-resource{}",
                resource.path().trim_end_matches('/')
            ));
            candidates.push(path);
            let mut root = resource.clone();
            root.set_path("/.well-known/oauth-protected-resource");
            if candidates[0] != root {
                candidates.push(root)
            }
        }
        let mut protected = None;
        for url in candidates {
            match self.get(url, ctx) {
                Ok(v) => {
                    protected = Some(v);
                    break;
                }
                Err(Error::Status(404)) => {}
                Err(e) => return Err(e),
            }
        }
        let protected = protected.ok_or(Error::Invalid)?;
        if protected["resource"].as_str() != Some(&t.resource)
            || !strings(&protected["authorization_servers"])?.contains(&t.settings.issuer)
        {
            return Err(Error::Invalid);
        }
        if let Some(methods) = protected.get("bearer_methods_supported")
            && !strings(methods)?.iter().any(|s| s == "header")
        {
            return Err(Error::Invalid);
        }
        let issuer = endpoint(&t.settings.issuer)?;
        let path = issuer.path().trim_end_matches('/');
        let mut candidates = Vec::new();
        let mut oauth = issuer.clone();
        oauth.set_path(&format!("/.well-known/oauth-authorization-server{path}"));
        candidates.push(oauth);
        if !path.is_empty() {
            let mut oidc = issuer.clone();
            oidc.set_path(&format!("/.well-known/openid-configuration{path}"));
            candidates.push(oidc);
        }
        let mut oidc = issuer.clone();
        oidc.set_path(&format!("{path}/.well-known/openid-configuration"));
        if !candidates.contains(&oidc) {
            candidates.push(oidc)
        }
        let mut document = None;
        for url in candidates {
            match self.get(url, ctx) {
                Ok(v) => {
                    document = Some(v);
                    break;
                }
                Err(Error::Status(404)) => {}
                Err(e) => return Err(e),
            }
        }
        let v = document.ok_or(Error::Invalid)?;
        if !strings(&v["code_challenge_methods_supported"])?
            .iter()
            .any(|s| s == "S256")
            || !strings(&v["response_types_supported"])?
                .iter()
                .any(|s| s == "code")
        {
            return Err(Error::Invalid);
        }
        let auth = if t.settings.client_secret_env.is_some() {
            "client_secret_post"
        } else {
            "none"
        };
        if !strings(&v["token_endpoint_auth_methods_supported"])?
            .iter()
            .any(|s| s == auth)
        {
            return Err(Error::Invalid);
        }
        let metadata = Metadata {
            issuer: string(&v, "issuer")?,
            authorization_endpoint: string(&v, "authorization_endpoint")?,
            token_endpoint: string(&v, "token_endpoint")?,
            require_iss: v
                .get("authorization_response_iss_parameter_supported")
                .map(|v| v.as_bool().ok_or(Error::Invalid))
                .transpose()?
                .unwrap_or(false),
        };
        metadata.validate(t)?;
        Ok(metadata)
    }
    pub fn exchange(
        &self,
        t: &Target,
        m: &Metadata,
        mut form: Vec<(String, String)>,
        old: Option<&Credential>,
        ctx: &TurnContext,
    ) -> Result<Credential, Error> {
        ctx.remaining()?;
        m.validate(t)?;
        form.extend([
            ("client_id".into(), t.settings.client_id.clone()),
            ("resource".into(), t.resource.clone()),
        ]);
        if let Some(selector) = &t.settings.client_secret_env {
            let secret = std::env::var(selector).map_err(|_| Error::Invalid)?;
            if !token(&secret) {
                return Err(Error::Invalid);
            }
            form.push(("client_secret".into(), secret));
        }
        let r = self.send(
            self.http
                .post(endpoint(&m.token_endpoint)?)
                .header("Accept", "application/json")
                .form(&form),
            ctx,
        )?;
        let v = self.json(r, ctx)?;
        if !v["token_type"]
            .as_str()
            .is_some_and(|s| s.eq_ignore_ascii_case("bearer"))
        {
            return Err(Error::Invalid);
        }
        let access_token = string(&v, "access_token")?;
        let refresh_token = v
            .get("refresh_token")
            .map(|s| s.as_str().map(String::from).ok_or(Error::Invalid))
            .transpose()?
            .or_else(|| old.and_then(|c| c.refresh_token.clone()));
        let seconds = v["expires_in"]
            .as_u64()
            .filter(|n| *n > 0 && *n <= 366 * 86400)
            .ok_or(Error::Invalid)?;
        let expires_ms = now_ms().checked_add(seconds * 1000).ok_or(Error::Invalid)?;
        let scopes = match v.get("scope") {
            Some(v) => v
                .as_str()
                .ok_or(Error::Invalid)?
                .split(' ')
                .map(String::from)
                .collect(),
            None => old.map_or_else(|| t.settings.scopes.clone(), |c| c.scopes.clone()),
        };
        if scopes.iter().any(|s| !scope(s))
            || scopes.iter().collect::<BTreeSet<_>>().len() != scopes.len()
        {
            return Err(Error::Invalid);
        }
        let credential = Credential {
            version: 1,
            binding: t.binding.clone(),
            metadata: m.clone(),
            access_token,
            refresh_token,
            expires_ms,
            scopes,
            uncertain: false,
        };
        credential.validate(t)?;
        if let Some(old) = old
            && credential.scopes.iter().any(|s| !old.scopes.contains(s))
        {
            return Err(Error::Invalid);
        }
        Ok(credential)
    }
}
fn string(v: &Value, key: &str) -> Result<String, Error> {
    v[key]
        .as_str()
        .filter(|s| s.len() <= 16384)
        .map(String::from)
        .ok_or(Error::Invalid)
}
fn strings(v: &Value) -> Result<Vec<String>, Error> {
    let a = v
        .as_array()
        .filter(|a| a.len() <= 64)
        .ok_or(Error::Invalid)?;
    a.iter()
        .map(|s| {
            s.as_str()
                .filter(|s| s.len() <= 4096)
                .map(String::from)
                .ok_or(Error::Invalid)
        })
        .collect()
}
/// Bounded auth-param parser: comma inside quoted realm is not a separator.
pub(super) fn challenge(header: &str) -> Result<(Option<String>, Option<String>), Error> {
    if header.len() > 8192 || !header.bytes().all(|b| (32..127).contains(&b)) {
        return Err(Error::Invalid);
    }
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (i, c) in header.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quoted && c == '\\' {
            escaped = true;
            continue;
        }
        if c == '"' {
            quoted = !quoted
        }
        if c == ',' && !quoted {
            parts.push(header[start..i].trim());
            start = i + 1;
        }
    }
    if quoted || escaped {
        return Err(Error::Invalid);
    }
    parts.push(header[start..].trim());
    let mut active = false;
    let mut seen = BTreeSet::new();
    let mut metadata = None;
    let mut scopes = None;
    for part in parts {
        let mut part = part;
        if part
            .get(..7)
            .is_some_and(|s| s.eq_ignore_ascii_case("Bearer "))
        {
            if active {
                return Err(Error::Invalid);
            }
            active = true;
            part = part[7..].trim();
        } else if part
            .split('=')
            .next()
            .is_some_and(|s| s.trim().contains(' '))
        {
            active = false;
        }
        if !active {
            continue;
        }
        let (key, value) = part.split_once('=').ok_or(Error::Invalid)?;
        let key = key.trim().to_ascii_lowercase();
        if !seen.insert(key.clone()) {
            return Err(Error::Invalid);
        }
        let value = value.trim();
        let value = if value.starts_with('"') {
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .ok_or(Error::Invalid)?;
            let mut out = String::new();
            let mut chars = value.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    out.push(chars.next().ok_or(Error::Invalid)?)
                } else if c == '"' {
                    return Err(Error::Invalid);
                } else {
                    out.push(c)
                }
            }
            out
        } else {
            if value.is_empty() || value.contains(' ') {
                return Err(Error::Invalid);
            }
            value.to_string()
        };
        match key.as_str() {
            "resource_metadata" => metadata = Some(value),
            "scope" => scopes = Some(value),
            _ => {}
        }
    }
    Ok((metadata, scopes))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn challenge_handles_quoted_comma_escape_and_other_scheme() {
        let (u,s)=challenge("Basic realm=\"old\", bEaReR realm=\"a,b\\\"c\", resource_metadata=\"https://mcp.test/meta\", scope=\"files:read\"").unwrap();
        assert_eq!(u.as_deref(), Some("https://mcp.test/meta"));
        assert_eq!(s.as_deref(), Some("files:read"));
    }
    #[test]
    fn malformed_duplicate_and_oversized_challenges_are_rejected() {
        for s in [
            "Bearer resource_metadata=\"unterminated",
            "Bearer scope=a,scope=b",
            "Bearer scope=\"a\"suffix",
            "Bearer scope=\"a\n\"",
        ] {
            assert!(challenge(s).is_err(), "{s}");
        }
        assert!(challenge(&"x".repeat(8193)).is_err());
        assert_eq!(challenge("Basic realm=\"x\"").unwrap(), (None, None));
    }
}
