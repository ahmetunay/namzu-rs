//! Independent loopback OAuth/MCP fixture. No accounts, browsers or model quota.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use namzu_core::{Cancellation, TurnContext};
use namzu_mcp::{
    config::{Document, Policy, Registry},
    manager::Manager,
    oauth::{Error, Settings, Target, Vault},
};
use namzu_session::PrivateStore;
use reqwest::Url;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
#[derive(Clone)]
struct Request {
    method: String,
    path: String,
    headers: String,
    body: String,
}
struct Server {
    origin: String,
    mode: String,
    log: Arc<Mutex<Vec<Request>>>,
    challenge: Arc<Mutex<String>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new(mode: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let log = Arc::new(Mutex::new(Vec::new()));
        let challenge = Arc::new(Mutex::new(String::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (o, m, l, c, s) = (
            origin.clone(),
            mode.to_string(),
            log.clone(),
            challenge.clone(),
            stop.clone(),
        );
        let worker = thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                let (mut socket, _) = match listener.accept() {
                    Ok(v) => v,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(e) => panic!("{e}"),
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let request = read(&mut socket);
                l.lock().unwrap().push(request.clone());
                let path = request.path.as_str();
                let mut headers = String::new();
                let mut status = 200;
                let value = if path == "/mcp" && request.method == "GET" {
                    status = 401;
                    if !m.starts_with("fallback") {
                        headers = format!(
                            "WWW-Authenticate: Bearer realm=\"fixture, realm\", resource_metadata=\"{o}/protected\", scope=\"files:read\"\r\n"
                        );
                    }
                    json!({})
                } else if path == "/protected"
                    || path.starts_with("/.well-known/oauth-protected-resource")
                {
                    if m == "fallback_root" && path.ends_with("/mcp") {
                        status = 404;
                        json!({})
                    } else if m == "oversized" {
                        json!({"padding":"x".repeat(70000)})
                    } else if m == "redirect" {
                        status = 302;
                        headers = format!("Location: {o}/do-not-follow\r\n");
                        json!({})
                    } else {
                        json!({"resource":if m=="wrong_resource" {format!("{o}/other")}else{format!("{o}/mcp")},"authorization_servers":[if m=="wrong_issuer" {format!("{o}/evil")}else if m=="issuer_path" {format!("{o}/tenant")}else{o.clone()}],"bearer_methods_supported":["header"]})
                    }
                } else if path == "/.well-known/oauth-authorization-server"
                    || path == "/.well-known/openid-configuration"
                    || path == "/.well-known/oauth-authorization-server/tenant"
                    || path == "/.well-known/openid-configuration/tenant"
                    || path == "/tenant/.well-known/openid-configuration"
                {
                    if (m == "fallback_oidc" && path.ends_with("oauth-authorization-server"))
                        || m == "issuer_path" && path != "/tenant/.well-known/openid-configuration"
                    {
                        status = 404;
                        json!({})
                    } else {
                        json!({"issuer":if m=="issuer_path"{format!("{o}/tenant")}else{o.clone()},"authorization_endpoint":if m=="external_auth"{"https://untrusted.invalid/auth".into()}else{format!("{o}/authorize")},"token_endpoint":format!("{o}/token"),"response_types_supported":["code"],"code_challenge_methods_supported":[if m=="plain_pkce"{"plain"}else{"S256"}],"token_endpoint_auth_methods_supported":["none","client_secret_post"],"authorization_response_iss_parameter_supported":true})
                    }
                } else if path == "/token" {
                    let form: BTreeMap<_, _> =
                        Url::parse(&format!("http://fixture/?{}", request.body))
                            .unwrap()
                            .query_pairs()
                            .map(|(k, v)| (k.into_owned(), v.into_owned()))
                            .collect();
                    assert_eq!(form["client_id"], "fixture-client");
                    assert_eq!(form["resource"], format!("{o}/mcp"));
                    if form["grant_type"] == "authorization_code" {
                        assert_eq!(form["code"], "fixture-code");
                        assert_eq!(
                            URL_SAFE_NO_PAD
                                .encode(Sha256::digest(form["code_verifier"].as_bytes())),
                            *c.lock().unwrap()
                        );
                    } else {
                        assert_eq!(form["grant_type"], "refresh_token");
                        assert_eq!(form["refresh_token"], "refresh-one");
                    }
                    if m == "refresh_disconnect" && form["grant_type"] == "refresh_token" {
                        drop(socket);
                        continue;
                    }
                    if m == "refresh_error" && form["grant_type"] == "refresh_token" {
                        status = 400;
                        json!({"error":"invalid_grant","error_description":"DO_NOT_PRINT_SECRET"})
                    } else {
                        json!({"access_token":if form["grant_type"]=="refresh_token"{"access-two"}else{"access-one"},"refresh_token":if form["grant_type"]=="refresh_token"{"refresh-two"}else{"refresh-one"},"token_type":if m=="wrong_type"{"MAC"}else{"Bearer"},"expires_in":if (m=="refresh"||m=="refresh_error"||m=="refresh_disconnect") && form["grant_type"]=="authorization_code"{1}else{3600},"scope":if m=="scope_increase"{"files:read admin"}else{"files:read"}})
                    }
                } else if path == "/mcp" && request.method == "POST" {
                    assert!(
                        request
                            .headers
                            .to_ascii_lowercase()
                            .contains("authorization: bearer access-")
                    );
                    let body: Value = serde_json::from_str(&request.body).unwrap();
                    match body["method"].as_str().unwrap() {
                        "initialize" => {
                            json!({"jsonrpc":"2.0","id":body["id"],"result":{"protocolVersion":"2025-11-25","capabilities":{},"serverInfo":{"name":"oauth-peer","version":"1"}}})
                        }
                        "notifications/initialized" => {
                            status = 202;
                            Value::Null
                        }
                        "ping" => json!({"jsonrpc":"2.0","id":body["id"],"result":{}}),
                        _ => panic!("unexpected method"),
                    }
                } else if path == "/mcp" && request.method == "DELETE" {
                    Value::Null
                } else {
                    status = 404;
                    json!({})
                };
                let bytes = if status == 202 {
                    Vec::new()
                } else {
                    serde_json::to_vec(&value).unwrap()
                };
                let _ = write!(
                    socket,
                    "HTTP/1.1 {status} fixture\r\n{headers}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                );
                let _ = socket.write_all(&bytes);
            }
        });
        Self {
            origin,
            mode: mode.into(),
            log,
            challenge,
            stop,
            worker: Some(worker),
        }
    }
    fn target(&self) -> Target {
        Target::new(format!("{}/mcp", self.origin), self.settings()).unwrap()
    }
    fn settings(&self) -> Settings {
        Settings {
            issuer: if self.mode == "issuer_path" {
                format!("{}/tenant", self.origin)
            } else {
                self.origin.clone()
            },
            client_id: "fixture-client".into(),
            scopes: vec!["files:read".into()],
            client_secret_env: None,
        }
    }
    fn count(&self, path: &str) -> usize {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.path == path)
            .count()
    }
    fn login(&self, v: &Vault, t: &Target) {
        let browser = Arc::new(Mutex::new(None));
        let output = browser.clone();
        let challenge = self.challenge.clone();
        let issuer = self.settings().issuer;
        let resource = format!("{}/mcp", self.origin);
        v.login(t, 0, &ctx(), move |url| {
            let url = Url::parse(url).unwrap();
            let q: BTreeMap<_, _> = url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            assert_eq!(q["code_challenge_method"], "S256");
            assert_eq!(q["client_id"], "fixture-client");
            assert_eq!(q["state"].len(), 43);
            assert_eq!(q["code_challenge"].len(), 43);
            assert_eq!(q["resource"], resource);
            *challenge.lock().unwrap() = q["code_challenge"].clone();
            let mut callback = Url::parse(&q["redirect_uri"]).unwrap();
            callback.query_pairs_mut().extend_pairs([
                ("state", q["state"].as_str()),
                ("code", "fixture-code"),
                ("iss", issuer.as_str()),
            ]);
            *output.lock().unwrap() = Some(thread::spawn(move || send_callback(&callback)));
        })
        .unwrap();
        browser.lock().unwrap().take().unwrap().join().unwrap();
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap();
    }
}
fn read(socket: &mut TcpStream) -> Request {
    let mut bytes = Vec::new();
    let mut b = [0];
    while !bytes.ends_with(b"\r\n\r\n") {
        socket.read_exact(&mut b).unwrap();
        bytes.push(b[0]);
        assert!(bytes.len() < 16384)
    }
    let headers = String::from_utf8(bytes).unwrap();
    let length = headers
        .lines()
        .find_map(|s| {
            s.to_ascii_lowercase()
                .strip_prefix("content-length: ")
                .map(|s| s.parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    let mut body = vec![0; length];
    socket.read_exact(&mut body).unwrap();
    let words: Vec<_> = headers.lines().next().unwrap().split(' ').collect();
    Request {
        method: words[0].into(),
        path: words[1].into(),
        headers,
        body: String::from_utf8(body).unwrap(),
    }
}
fn send_callback(url: &Url) {
    let mut socket = TcpStream::connect(("127.0.0.1", url.port().unwrap())).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    write!(
        socket,
        "GET {}?{} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
        url.path(),
        url.query().unwrap(),
        url.port().unwrap()
    )
    .unwrap();
    let mut result = String::new();
    socket.read_to_string(&mut result).unwrap();
    assert!(result.starts_with("HTTP/1.1 200"));
}
fn ctx() -> TurnContext {
    TurnContext::new(Duration::from_secs(10), Cancellation::default()).unwrap()
}
fn vault(root: &Path) -> Vault {
    Vault::new(PrivateStore::open(root).unwrap())
}
#[test]
fn pkce_login_private_storage_status_logout_and_target_binding() {
    let root = tempfile::tempdir().unwrap();
    let s = Server::new("normal");
    let v = vault(root.path());
    let t = s.target();
    s.login(&v, &t);
    assert!(v.status(&t).unwrap().logged_in);
    assert_eq!(v.access_token(&t, &ctx()).unwrap(), "access-one");
    assert_eq!(s.count("/token"), 1);
    let mut settings = s.settings();
    settings.client_id = "different-client".into();
    let other = Target::new(format!("{}/mcp", s.origin), settings).unwrap();
    assert!(matches!(
        v.access_token(&other, &ctx()),
        Err(Error::LoginRequired)
    ));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for file in std::fs::read_dir(root.path()).unwrap() {
            assert_eq!(
                file.unwrap().metadata().unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    v.logout(&t).unwrap();
    assert!(!v.status(&t).unwrap().logged_in);
}
#[test]
fn expiry_refresh_rotates_atomically_and_next_access_does_not_refresh() {
    let root = tempfile::tempdir().unwrap();
    let s = Server::new("refresh");
    let v = vault(root.path());
    let t = s.target();
    s.login(&v, &t);
    assert!(!v.status(&t).unwrap().logged_in);
    assert_eq!(v.access_token(&t, &ctx()).unwrap(), "access-two");
    assert_eq!(v.access_token(&t, &ctx()).unwrap(), "access-two");
    assert_eq!(s.count("/token"), 2);
    let bytes = std::fs::read_dir(root.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|s| s == "json"))
        .unwrap();
    let value: Value = serde_json::from_slice(&std::fs::read(bytes).unwrap()).unwrap();
    assert_eq!(value["refresh_token"], "refresh-two");
}
#[test]
fn failed_refresh_is_durable_uncertain_and_cannot_replay() {
    let root = tempfile::tempdir().unwrap();
    let s = Server::new("refresh_error");
    let t = s.target();
    let v = vault(root.path());
    s.login(&v, &t);
    assert!(matches!(
        v.access_token(&t, &ctx()),
        Err(Error::Status(400))
    ));
    drop(v);
    let v = vault(root.path());
    assert!(matches!(v.access_token(&t, &ctx()), Err(Error::Uncertain)));
    assert!(v.status(&t).unwrap().uncertain);
    assert_eq!(s.count("/token"), 2);
}
#[test]
fn protected_resource_and_oidc_well_known_fallbacks() {
    for mode in [
        "fallback_path",
        "fallback_root",
        "fallback_oidc",
        "issuer_path",
    ] {
        let root = tempfile::tempdir().unwrap();
        let s = Server::new(mode);
        let v = vault(root.path());
        s.login(&v, &s.target());
        assert!(v.status(&s.target()).unwrap().logged_in);
        if mode == "fallback_root" {
            assert_eq!(s.count("/.well-known/oauth-protected-resource/mcp"), 1);
            assert_eq!(s.count("/.well-known/oauth-protected-resource"), 1);
        }
        if mode == "fallback_oidc" {
            assert_eq!(s.count("/.well-known/openid-configuration"), 1);
        }
    }
}
#[test]
fn bad_metadata_is_rejected_before_browser_or_token_exchange() {
    for mode in [
        "wrong_resource",
        "wrong_issuer",
        "external_auth",
        "plain_pkce",
        "oversized",
        "redirect",
    ] {
        let root = tempfile::tempdir().unwrap();
        let s = Server::new(mode);
        let v = vault(root.path());
        assert!(
            v.login(&s.target(), 0, &ctx(), |_| panic!("browser must not open"))
                .is_err(),
            "{mode}"
        );
        assert_eq!(s.count("/token"), 0);
        assert_eq!(s.count("/do-not-follow"), 0);
        assert!(!v.status(&s.target()).unwrap().logged_in);
    }
}
#[test]
fn token_scope_expansion_or_wrong_type_cannot_publish_credentials() {
    for mode in ["scope_increase", "wrong_type"] {
        let root = tempfile::tempdir().unwrap();
        let s = Server::new(mode);
        let v = vault(root.path());
        let t = s.target();
        let challenge = s.challenge.clone();
        let issuer = s.origin.clone();
        let mut browser = None;
        let result = v.login(&t, 0, &ctx(), |url| {
            let u = Url::parse(url).unwrap();
            let q: BTreeMap<_, _> = u
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            *challenge.lock().unwrap() = q["code_challenge"].clone();
            let mut cb = Url::parse(&q["redirect_uri"]).unwrap();
            cb.query_pairs_mut().extend_pairs([
                ("state", q["state"].as_str()),
                ("code", "fixture-code"),
                ("iss", issuer.as_str()),
            ]);
            browser = Some(thread::spawn(move || send_callback(&cb)));
        });
        browser.unwrap().join().unwrap();
        assert!(matches!(result, Err(Error::Invalid)));
        assert!(!v.status(&t).unwrap().logged_in);
    }
}
#[test]
fn manager_uses_stored_token_and_never_starts_login_implicitly() {
    let root = tempfile::tempdir().unwrap();
    let s = Server::new("normal");
    let r = Registry::open(root.path()).unwrap();
    r.replace(Document::parse(json!({"version":1,"default_profile":"default","profiles":{"default":{"servers":{"fixture":{"transport":{"type":"http","url":format!("{}/mcp",s.origin)},"oauth":s.settings(),"policy":{"require_approval":false}}}}}}).to_string().as_bytes()).unwrap()).unwrap();
    let context = ctx();
    let mut m = Manager::new(
        &r,
        None,
        None,
        Policy {
            require_approval: false,
            ..Policy::default()
        },
        &context,
    )
    .unwrap();
    let p = m.prepare_connect("fixture").unwrap().permit().unwrap();
    assert!(m.execute(p, &context).is_err());
    assert_eq!(s.count("/mcp"), 0);
    let v = Vault::new(PrivateStore::open(root.path().join("mcp/oauth")).unwrap());
    s.login(&v, &s.target());
    let p = m.prepare_connect("fixture").unwrap().permit().unwrap();
    m.execute(p, &context).unwrap();
    let p = m
        .prepare_call("fixture", "ping", None)
        .unwrap()
        .permit()
        .unwrap();
    m.execute(p, &context).unwrap();
    m.close(&context).unwrap();
    assert_eq!(s.count("/token"), 1);
    assert_eq!(
        s.log
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method == "POST" && r.path == "/mcp")
            .count(),
        3
    );
}
#[test]
fn invalid_settings_and_conflicting_config_fail_before_network() {
    let s = Server::new("normal");
    for issuer in [
        "http://example.com",
        "https://u:p@example.com",
        "https://example.com?q=x",
        "https://example.com/#fragment",
    ] {
        let mut settings = s.settings();
        settings.issuer = issuer.into();
        assert!(settings.validate().is_err());
    }
    let mut settings = s.settings();
    settings.scopes.push("files:read".into());
    assert!(settings.validate().is_err());
    for transport in [
        json!({"type":"stdio","program":"/bin/true","cwd":"/"}),
        json!({"type":"http","url":format!("{}/mcp",s.origin),"bearer_env":"FIXTURE_TOKEN"}),
    ] {
        assert!(Document::parse(json!({"version":1,"default_profile":"default","profiles":{"default":{"servers":{"fixture":{"transport":transport,"oauth":s.settings()}}}}}).to_string().as_bytes()).is_err());
    }
    assert_eq!(s.log.lock().unwrap().len(), 0);
}
#[test]
fn cancellation_at_callback_never_sends_code_or_creates_credential() {
    let root = tempfile::tempdir().unwrap();
    let s = Server::new("normal");
    let v = vault(root.path());
    let t = s.target();
    let cancel = Cancellation::default();
    let context = TurnContext::new(Duration::from_secs(300), cancel.clone()).unwrap();
    assert!(matches!(
        v.login(&t, 0, &context, |_| cancel.cancel()),
        Err(Error::Context(namzu_core::AgentError::Cancelled))
    ));
    assert_eq!(s.count("/token"), 0);
    assert!(!v.status(&t).unwrap().logged_in);
}
#[test]
fn pre_cancelled_oauth_does_no_network_or_login() {
    let root = tempfile::tempdir().unwrap();
    let s = Server::new("normal");
    let v = vault(root.path());
    let cancel = Cancellation::default();
    let context = TurnContext::new(Duration::from_secs(300), cancel.clone()).unwrap();
    cancel.cancel();
    assert!(
        v.login(&s.target(), 0, &context, |_| panic!("no browser"))
            .is_err()
    );
    assert_eq!(s.log.lock().unwrap().len(), 0);
}
#[test]
fn token_store_symlink_is_not_followed() {
    let root = tempfile::tempdir().unwrap();
    let s = Server::new("normal");
    let v = vault(root.path());
    let t = s.target();
    s.login(&v, &t);
    let file = std::fs::read_dir(root.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|s| s == "json"))
        .unwrap();
    let protected = root.path().join("protected.json");
    std::fs::rename(&file, &protected).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&protected, &file).unwrap();
        assert!(v.access_token(&t, &ctx()).is_err());
        assert!(protected.exists());
    }
}
#[test]
fn lost_refresh_reply_cannot_replay_a_rotating_grant() {
    let root = tempfile::tempdir().unwrap();
    let s = Server::new("refresh_disconnect");
    let v = vault(root.path());
    let t = s.target();
    s.login(&v, &t);
    assert!(matches!(v.access_token(&t, &ctx()), Err(Error::Network)));
    assert!(matches!(v.access_token(&t, &ctx()), Err(Error::Uncertain)));
    assert_eq!(s.count("/token"), 2);
}
