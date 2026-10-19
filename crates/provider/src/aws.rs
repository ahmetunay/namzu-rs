//! Explicit static/session AWS credentials and SigV4; no profile/IMDS/login discovery.
use namzu_core::AgentError;
use openssl::{hash::MessageDigest, pkey::PKey, sign::Signer};
use reqwest::{
    Url,
    header::{HeaderMap, HeaderValue},
};
use sha2::{Digest, Sha256};

pub struct Credentials {
    pub(crate) region: String,
    access: String,
    secret: String,
    token: Option<String>,
}
impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credentials(<redacted>)")
    }
}
impl Credentials {
    pub fn new(
        region: String,
        access: String,
        secret: String,
        token: Option<String>,
    ) -> Result<Self, AgentError> {
        if region.is_empty()
            || region.len() > 64
            || !region
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || access.is_empty()
            || access.len() > 256
            || !access.bytes().all(|b| b.is_ascii_alphanumeric())
            || secret.is_empty()
            || secret.len() > 4096
            || secret.chars().any(char::is_control)
            || token.as_ref().is_some_and(|v| {
                v.is_empty() || v.len() > 16384 || HeaderValue::from_str(v).is_err()
            })
        {
            return Err(AgentError::Configuration(
                "invalid explicit AWS credentials",
            ));
        }
        Ok(Self {
            region,
            access,
            secret,
            token,
        })
    }
    pub(crate) fn tag(&self) -> String {
        let encoded =
            serde_json::to_vec(&(&self.region, &self.access, &self.secret, &self.token)).unwrap();
        hex(&encoded)
    }
    pub(crate) fn headers(
        &self,
        url: &Url,
        body: &[u8],
        timestamp: &str,
    ) -> Result<HeaderMap, AgentError> {
        if !timestamp.is_ascii()
            || timestamp.len() != 16
            || &timestamp[8..9] != "T"
            || !timestamp[..8]
                .bytes()
                .chain(timestamp[9..15].bytes())
                .all(|b| b.is_ascii_digit())
            || !timestamp.ends_with('Z')
        {
            return Err(AgentError::Configuration("invalid AWS signing timestamp"));
        }
        let host = url[url::Position::BeforeHost..url::Position::AfterPort].to_owned();
        let payload = hex(body);
        let mut canonical = std::collections::BTreeMap::from([
            ("host", host),
            ("x-amz-content-sha256", payload.clone()),
            ("x-amz-date", timestamp.to_owned()),
        ]);
        if let Some(token) = &self.token {
            canonical.insert("x-amz-security-token", token.clone());
        }
        let names = canonical.keys().copied().collect::<Vec<_>>().join(";");
        let lines = canonical
            .iter()
            .map(|(k, v)| format!("{k}:{}\n", v.trim()))
            .collect::<String>();
        let path = url
            .path()
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-_.~/".contains(&b) {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect::<String>();
        let request = format!("POST\n{path}\n\n{lines}\n{names}\n{payload}");
        let scope = format!("{}/{}/bedrock/aws4_request", &timestamp[..8], self.region);
        let to_sign = format!(
            "AWS4-HMAC-SHA256\n{timestamp}\n{scope}\n{}",
            hex(request.as_bytes())
        );
        let key = hmac(
            format!("AWS4{}", self.secret).as_bytes(),
            &timestamp.as_bytes()[..8],
        )?;
        let key = hmac(&key, self.region.as_bytes())?;
        let key = hmac(&key, b"bedrock")?;
        let key = hmac(&key, b"aws4_request")?;
        let signature = hmac(&key, to_sign.as_bytes())?
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let mut headers = HeaderMap::new();
        for (name, value) in canonical {
            let mut v = HeaderValue::from_str(&value)
                .map_err(|_| AgentError::Configuration("invalid AWS header"))?;
            if name == "x-amz-security-token" {
                v.set_sensitive(true);
            }
            headers.insert(reqwest::header::HeaderName::from_static(name), v);
        }
        let mut auth = HeaderValue::from_str(&format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={names}, Signature={signature}",
            self.access
        ))
        .map_err(|_| AgentError::Configuration("invalid AWS authorization"))?;
        auth.set_sensitive(true);
        headers.insert(reqwest::header::AUTHORIZATION, auth);
        Ok(headers)
    }
}
fn hmac(key: &[u8], value: &[u8]) -> Result<Vec<u8>, AgentError> {
    let key = PKey::hmac(key).map_err(|_| AgentError::Protocol("AWS HMAC failed"))?;
    let mut signer = Signer::new(MessageDigest::sha256(), &key)
        .map_err(|_| AgentError::Protocol("AWS HMAC failed"))?;
    signer
        .update(value)
        .and_then(|_| signer.sign_to_vec())
        .map_err(|_| AgentError::Protocol("AWS HMAC failed"))
}
fn hex(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}
pub(crate) fn now() -> Result<String, AgentError> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| AgentError::Configuration("system clock before Unix epoch"))?
        .as_secs();
    let seconds: libc::time_t = seconds
        .try_into()
        .map_err(|_| AgentError::Configuration("AWS clock overflow"))?;
    let mut out = std::mem::MaybeUninit::<libc::tm>::uninit();
    // gmtime_r writes a complete tm on success; caller-owned storage is thread-safe.
    let out = unsafe {
        if libc::gmtime_r(&seconds, out.as_mut_ptr()).is_null() {
            return Err(AgentError::Configuration("AWS clock conversion failed"));
        }
        out.assume_init()
    };
    Ok(format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        out.tm_year + 1900,
        out.tm_mon + 1,
        out.tm_mday,
        out.tm_hour,
        out.tm_min,
        out.tm_sec
    ))
}
impl crate::OpenAiCompatible {
    pub fn for_bedrock(
        base_url: &str,
        model: String,
        credentials: Credentials,
        options: crate::services::Options,
    ) -> Result<Self, AgentError> {
        if options.bedrock_auth.as_deref() != Some("sigv4")
            || options.aws_region.as_deref() != Some(&credentials.region)
        {
            return Err(AgentError::Configuration(
                "SigV4 requires matching explicit Bedrock auth/region",
            ));
        }
        let mut p = Self::for_service(
            crate::services::Service::Bedrock,
            base_url,
            model,
            "sigv4-constructor",
            options,
        )?;
        let host = p.endpoint.host_str().unwrap_or_default();
        if host.ends_with(".amazonaws.com")
            && host != format!("bedrock-runtime.{}.amazonaws.com", credentials.region)
        {
            return Err(AgentError::Configuration(
                "Bedrock endpoint region differs from signing region",
            ));
        }
        p.credential_tag = credentials.tag();
        p.authorization = None;
        p.aws = Some(credentials);
        Ok(p)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rfc4231_hmac_and_redacted_credentials() {
        let signature = hmac(&[0x0b; 20], b"Hi There")
            .unwrap()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        assert_eq!(
            signature,
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        let c =
            Credentials::new("us-east-1".into(), "FIXTURE".into(), "secret".into(), None).unwrap();
        assert!(!format!("{c:?}").contains("secret"));
        assert_eq!(now().unwrap().len(), 16);
    }
}

#[cfg(test)]
mod signing_tests {
    use super::*;
    #[test]
    fn sigv4_matches_independent_fixed_vector_and_signs_session_token() {
        let c = Credentials::new(
            "us-east-1".into(),
            "FIXTUREACCESS".into(),
            "fixture-secret".into(),
            Some("fixture-token".into()),
        )
        .unwrap();
        let url = Url::parse(
            "https://bedrock-runtime.us-east-1.amazonaws.com/model/anthropic.fixture:0/converse",
        )
        .unwrap();
        let headers = c
            .headers(&url, br#"{"messages":[]}"#, "20261019T120000Z")
            .unwrap();
        assert!(headers["authorization"].to_str().unwrap().ends_with(
            "Signature=11f1e1e4c5e90ab9fecd6796a13eba0a9de985ffae79c9d177541e0316fa6a87"
        ));
        assert!(headers["authorization"].is_sensitive());
        assert!(headers["x-amz-security-token"].is_sensitive());
        assert_ne!(
            c.tag(),
            Credentials::new(
                "us-east-1".into(),
                "FIXTUREACCESS".into(),
                "other-secret".into(),
                Some("fixture-token".into())
            )
            .unwrap()
            .tag()
        );
        assert!(c.headers(&url, b"{}", "éééééééé").is_err());
    }
    #[test]
    fn sigv4_credentials_region_and_factory_options_are_explicit() {
        let options = crate::services::Options {
            bedrock_auth: Some("sigv4".into()),
            aws_region: Some("eu-west-1".into()),
            ..Default::default()
        };
        let c = Credentials::new(
            "eu-west-1".into(),
            "FIXTUREACCESS".into(),
            "fixture-secret".into(),
            None,
        )
        .unwrap();
        assert!(
            crate::OpenAiCompatible::for_bedrock(
                "https://bedrock-runtime.us-east-1.amazonaws.com",
                "fixture".into(),
                c,
                options
            )
            .is_err()
        );
        for access in ["", "bad/key", "bad\nheader"] {
            assert!(
                Credentials::new("us-east-1".into(), access.into(), "secret".into(), None).is_err()
            );
        }
    }
}
