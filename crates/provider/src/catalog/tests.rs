use super::*;
use crate::services::Options;
use namzu_core::{Cancellation, Message, Provider, ToolDefinition};
use std::{
    io::{Read, Write},
    net::TcpListener,
    time::{Duration, Instant},
};
fn ctx() -> TurnContext {
    TurnContext::new(Duration::from_secs(10), Cancellation::default()).unwrap()
}
fn record() -> Model {
    Model {
        id: "fixture".into(),
        context_window: Some(1000),
        input_token_limit: Some(900),
        output_token_limit: Some(100),
        ..Default::default()
    }
}
fn catalog(provider: &str, base: &str) -> Catalog {
    Catalog {
        version: 1,
        provider: provider.into(),
        base_url: canonical_base(base).unwrap(),
        captured_ms: 1,
        models: vec![record()],
    }
}
type Handler = Box<dyn FnOnce(String, String) -> (u16, String) + Send>;
fn serve(handlers: Vec<Handler>) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let worker = std::thread::spawn(move || {
        for handler in handlers {
            let until = Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < until, "missing catalogue request");
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = vec![];
            while !bytes.ends_with(b"\r\n\r\n") {
                let mut b = [0];
                stream.read_exact(&mut b).unwrap();
                bytes.push(b[0]);
                assert!(bytes.len() < 16384);
            }
            let headers = String::from_utf8(bytes).unwrap();
            assert!(headers.starts_with("GET "));
            let path = headers
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .to_owned();
            let (status, body) = handler(path, headers);
            write!(stream,"HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        }
    });
    (format!("http://{address}"), worker)
}
fn provider(service: Service, base: &str, model: &str) -> OpenAiCompatible {
    let mut p = OpenAiCompatible::for_service(
        service,
        base,
        model.into(),
        if service.key_optional() {
            ""
        } else {
            "fixture-key"
        },
        Options::default(),
    )
    .unwrap();
    p.client = crate::http::Client::builder().no_proxy().build().unwrap();
    p
}
#[test]
fn reported_limits_are_distinct_and_unknown_is_not_zero() {
    let m = model(
        &json!({"name":"models/fixture","inputTokenLimit":900,"outputTokenLimit":100}),
        "google",
    )
    .unwrap();
    assert_eq!(m.context_window, None);
    assert_eq!(m.input_token_limit, Some(900));
    assert_eq!(m.output_token_limit, Some(100));
    let m = model(&json!({"id":"fixture"}), "openai-compatible").unwrap();
    assert_eq!(m.context_window, None);
    assert_eq!(m.tools, None);
    assert_eq!(m.images, None);
    for n in [
        json!(0),
        json!(-1),
        json!(0.5),
        json!("1000"),
        json!(1_000_000_001u64),
    ] {
        assert!(model(&json!({"id":"fixture","context_length":n}), "openrouter").is_err());
    }
}
#[test]
fn narrower_reported_context_output_and_modalities_are_preserved() {
    let m=model(&json!({"id":"fixture","context_length":2000,"top_provider":{"context_length":1000,"max_completion_tokens":80},"max_output_tokens":100,"architecture":{"input_modalities":["text","image"]},"supported_parameters":["tools"]}),"openrouter").unwrap();
    assert_eq!(m.context_window, Some(1000));
    assert_eq!(m.output_token_limit, Some(80));
    assert_eq!(m.tools, Some(true));
    assert_eq!(m.images, Some(true));
    assert!(
        model(
            &json!({"id":"fixture","supported_parameters":[false]}),
            "openrouter"
        )
        .is_err()
    );
}
#[test]
fn measured_context_tokens_output_defaults_and_overflow_are_checked() {
    let m = record();
    assert_eq!(m.output_budget(None).unwrap(), 100);
    assert!(m.output_budget(Some(101)).is_err());
    assert!(m.validate_tokens(Some(900), 100).is_ok());
    assert!(m.validate_tokens(Some(901), 10).is_err());
    assert!(m.validate_tokens(Some(u64::MAX), 1).is_err());
    assert!(m.validate_tokens(None, 0).is_err());
    let mut m = m;
    m.input_token_limit = None;
    assert!(m.validate_tokens(Some(950), 100).is_err());
    assert!(m.validate_tokens(None, 100).is_ok());
}
#[test]
fn duplicate_ids_aliases_malformed_names_and_excessive_rows_rejected() {
    let mut c = catalog("bedrock", "http://127.0.0.1:1");
    c.models[0].aliases.push("arn:fixture".into());
    c.models.push(Model {
        id: "arn:fixture".into(),
        ..Default::default()
    });
    assert!(c.validate().is_err());
    let mut c = catalog("openrouter", "http://127.0.0.1:1");
    c.models[0].id = "bad\nname".into();
    assert!(c.validate().is_err());
    assert!(rows(&json!({"data":vec![json!({"id":"x"});4097]}), "openrouter").is_err());
    assert!(serde_json::from_value::<Model>(json!({"id":"x","fallback_model":"paid"})).is_err());
}
#[test]
fn google_pages_keep_same_origin_encoded_cursor_and_exclude_embedding_models() {
    let (url, worker) = serve(vec![
        Box::new(|path, h| {
            assert_eq!(path, "/models?pageSize=1000");
            assert!(
                h.to_ascii_lowercase()
                    .contains("x-goog-api-key: fixture-key")
            );
            (200,json!({"models":[{"name":"models/fixture","inputTokenLimit":900,"outputTokenLimit":100,"supportedGenerationMethods":["generateContent"]},{"name":"models/embedding","supportedGenerationMethods":["embedContent"]}],"nextPageToken":"https://other.invalid/?key=x&next=yes"}).to_string())
        }),
        Box::new(|path, _| {
            assert!(path.starts_with("/models?pageSize=1000&pageToken=https%3A%2F%2Fother.invalid%2F%3Fkey%3Dx%26next%3Dyes"));
            (200,json!({"models":[{"name":"models/second","supportedGenerationMethods":["generateContent"]}]}).to_string())
        }),
    ]);
    let p = provider(Service::Google, &url, "fixture");
    let c = p.discover_models(&ctx()).unwrap();
    assert_eq!(c.models.len(), 2);
    assert_eq!(c.models[0].id, "fixture");
    worker.join().unwrap();
}
#[test]
fn anthropic_pagination_and_native_zen_discovery_use_correct_auth() {
    let (url, worker) = serve(vec![
        Box::new(|path, h| {
            assert_eq!(path, "/models?limit=1000");
            assert!(h.to_ascii_lowercase().contains("x-api-key: fixture-key"));
            (
                200,
                json!({"data":[{"id":"first"}],"has_more":true,"last_id":"first"}).to_string(),
            )
        }),
        Box::new(|path, _| {
            assert_eq!(path, "/models?limit=1000&after_id=first");
            (
                200,
                json!({"data":[{"id":"second"}],"has_more":false}).to_string(),
            )
        }),
    ]);
    assert_eq!(
        provider(Service::Anthropic, &url, "fixture")
            .discover_models(&ctx())
            .unwrap()
            .models
            .len(),
        2
    );
    worker.join().unwrap();
    let (url, worker) = serve(vec![Box::new(|path, h| {
        assert_eq!(path, "/models");
        assert!(
            h.to_ascii_lowercase()
                .contains("authorization: bearer fixture-key")
        );
        (200, json!({"data":[{"id":"claude-fixture"}]}).to_string())
    })]);
    provider(Service::Zen, &url, "claude-fixture")
        .discover_models(&ctx())
        .unwrap();
    worker.join().unwrap();
}
#[test]
fn failed_second_page_cycles_and_duplicates_never_publish_partial_cache() {
    for mode in ["duplicate", "cycle", "http"] {
        let mode = mode.to_owned();
        let (url, worker) = serve(vec![
            Box::new(|_, _| {
                (
                    200,
                    json!({"data":[{"id":"first"}],"has_more":true,"last_id":"first"}).to_string(),
                )
            }),
            Box::new(move |_, _| match mode.as_str() {
                "duplicate" => (
                    200,
                    json!({"data":[{"id":"first"}],"has_more":false}).to_string(),
                ),
                "cycle" => (
                    200,
                    json!({"data":[],"has_more":true,"last_id":"first"}).to_string(),
                ),
                _ => (429, "PRIVATE_SERVER_BODY".into()),
            }),
        ]);
        let error = provider(Service::Anthropic, &url, "fixture")
            .discover_models(&ctx())
            .unwrap_err();
        assert!(!error.to_string().contains("PRIVATE_SERVER_BODY"));
        worker.join().unwrap();
    }
    assert!(next(&json!({"has_more":true}), "anthropic").is_err());
    assert!(next(&json!({"has_more":true,"last_id":"a"}), "openrouter").is_err());
}
#[test]
fn local_and_bedrock_catalogues_use_native_paths_and_report_unknown_limits() {
    for (service, path, body) in [
        (
            Service::Ollama,
            "/api/tags",
            json!({"models":[{"name":"fixture"}]}),
        ),
        (
            Service::Lmstudio,
            "/models",
            json!({"data":[{"id":"fixture","max_context_length":2048}]}),
        ),
        (
            Service::Bedrock,
            "/foundation-models",
            json!({"modelSummaries":[{"modelId":"fixture","modelArn":"arn:fixture","inputModalities":["TEXT","IMAGE"]}]}),
        ),
    ] {
        let (url, worker) = serve(vec![Box::new(move |actual, h| {
            assert_eq!(actual, path);
            if service.key_optional() {
                assert!(!h.to_ascii_lowercase().contains("authorization:"));
            }
            (200, body.to_string())
        })]);
        let c = provider(service, &url, "fixture")
            .discover_models(&ctx())
            .unwrap();
        assert_eq!(c.models[0].id, "fixture");
        if service == Service::Bedrock {
            assert_eq!(c.select("arn:fixture").unwrap().images, Some(true));
        }
        worker.join().unwrap();
    }
}
#[test]
fn cache_integrity_identity_scope_and_key_rotation_are_checked() {
    let dir = tempfile::tempdir().unwrap();
    let store = PrivateStore::open(dir.path().join("state")).unwrap();
    let p = provider(Service::Openrouter, "http://127.0.0.1:1", "fixture");
    let c = catalog("openrouter", "http://127.0.0.1:1");
    let id = p.save_catalog(&store, c.clone(), "FIXTURE_KEY").unwrap();
    assert!(Cached::valid_id(&id));
    let saved = Cached::read(&store, &id).unwrap();
    p.require_catalog(&saved, "FIXTURE_KEY").unwrap();
    assert!(saved.catalog.select("missing").is_err());
    assert!(p.require_catalog(&saved, "OTHER_KEY").is_err());
    let changed = OpenAiCompatible::for_service(
        Service::Openrouter,
        "http://127.0.0.1:1",
        "fixture".into(),
        "other-key",
        Options::default(),
    )
    .unwrap();
    assert!(changed.require_catalog(&saved, "FIXTURE_KEY").is_err());
    let raw = serde_json::to_string(&saved).unwrap();
    assert!(!raw.contains("fixture-key"));
    assert!(!format!("{saved:?}").contains("credential_tag"));
    let mut tampered = saved;
    tampered.catalog.models[0].output_token_limit = Some(99);
    store
        .child("catalogs")
        .unwrap()
        .write(&format!("{id}.json"), &tampered)
        .unwrap();
    assert!(Cached::read(&store, &id).is_err());
    assert!(!Cached::valid_id("../catalogue"));
}
#[test]
fn content_selection_id_survives_refresh_time_but_changes_with_model_data() {
    let dir = tempfile::tempdir().unwrap();
    let store = PrivateStore::open(dir.path().join("state")).unwrap();
    let p = provider(Service::Openrouter, "http://127.0.0.1:1", "fixture");
    let mut c = catalog("openrouter", "http://127.0.0.1:1");
    let first = p.save_catalog(&store, c.clone(), "KEY").unwrap();
    c.captured_ms = 2;
    assert_eq!(first, p.save_catalog(&store, c.clone(), "KEY").unwrap());
    c.models[0].output_token_limit = Some(90);
    assert_ne!(first, p.save_catalog(&store, c, "KEY").unwrap());
}
#[test]
fn selected_limits_and_known_unsupported_tools_fail_before_http() {
    let mut m = record();
    m.tools = Some(false);
    m.images = Some(false);
    let mut p = provider(Service::Openrouter, "http://127.0.0.1:1", "fixture")
        .with_model(m)
        .unwrap();
    assert_eq!(p.model_metadata().unwrap().output_token_limit, Some(100));
    assert_eq!(p.options.max_output_tokens, Some(100));
    let tools = [ToolDefinition {
        name: "read".into(),
        description: "fixture".into(),
        parameters: json!({"type":"object"}),
    }];
    assert!(matches!(
        p.complete(
            &[Message::User {
                content: "fixture".into()
            }],
            &tools,
            &ctx()
        ),
        Err(AgentError::Configuration(_))
    ));
    assert!(p.take_metadata().is_none());
    assert!(
        provider(Service::Openrouter, "http://127.0.0.1:1", "other-model")
            .with_model(record())
            .is_err()
    );
}
#[test]
fn codex_hidden_models_and_reported_metadata_are_normalized_without_api_fallback() {
    let c=codex_catalog(&json!({"models":[{"slug":"fixture","display_name":"Fixture","context_window":1000},{"slug":"hidden","visibility":"hide"}]}),"https://chatgpt.com/backend-api/codex/").unwrap();
    assert_eq!(c.models.len(), 1);
    assert_eq!(c.models[0].context_window, Some(1000));
    assert!(
        codex_catalog(
            &json!({"models":[{"slug":"fixture"},{"slug":"fixture"}]}),
            "https://chatgpt.com/backend-api/codex/"
        )
        .is_err()
    );
}

#[test]
fn cancellation_before_catalogue_admission_sends_no_network_request() {
    let signal = Cancellation::default();
    signal.cancel();
    let ctx = TurnContext::new(Duration::from_secs(10), signal).unwrap();
    assert!(matches!(
        provider(Service::Openrouter, "http://127.0.0.1:1", "fixture").discover_models(&ctx),
        Err(AgentError::Cancelled)
    ));
}
