use namzu_core::{Message, ToolCall};
use namzu_provider::{OpenAiCompatible, decode_completion, encode_messages};
use serde_json::json;

fn response(message: serde_json::Value, finish: &str) -> serde_json::Value {
    json!({"choices":[{"message":message,"finish_reason":finish}]})
}

#[test]
fn tool_call_and_result_round_trip_preserves_protocol_ids() {
    let decoded=decode_completion(&response(json!({"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"x\"}"}}]}),"tool_calls")).unwrap();
    assert_eq!(decoded.tool_calls[0].arguments, json!({"path":"x"}));
    let encoded = encode_messages(&[
        Message::Assistant {
            content: decoded.text,
            tool_calls: decoded.tool_calls,
        },
        Message::Tool {
            tool_call_id: "c1".into(),
            content: "value".into(),
            is_error: false,
        },
    ]);
    assert_eq!(encoded[0]["tool_calls"][0]["id"], "c1");
    assert_eq!(
        encoded[0]["tool_calls"][0]["function"]["arguments"],
        "{\"path\":\"x\"}"
    );
    assert_eq!(encoded[1]["tool_call_id"], "c1");
    let result: serde_json::Value =
        serde_json::from_str(encoded[1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(result, json!({"content":"value","is_error":false}));
}

#[test]
fn truncated_or_filtered_completions_never_become_success() {
    for reason in ["length", "content_filter", "unknown"] {
        assert!(
            decode_completion(&response(
                json!({"role":"assistant","content":"partial"}),
                reason
            ))
            .is_err()
        );
    }
}

#[test]
fn malformed_arguments_and_mismatched_finish_reason_are_refused() {
    for args in ["{", "[]", "null"] {
        assert!(decode_completion(&response(json!({"role":"assistant","tool_calls":[{"id":"a","type":"function","function":{"name":"read_file","arguments":args}}]}),"tool_calls")).is_err());
    }
    assert!(
        decode_completion(&response(
            json!({"role":"assistant","content":"text"}),
            "tool_calls"
        ))
        .is_err()
    );
    assert!(decode_completion(&json!({"choices":[]})).is_err());
}

#[test]
fn unsafe_endpoints_are_rejected_before_any_network_call() {
    for url in [
        "http://example.com/v1",
        "https://user:pass@example.com/v1",
        "https://example.com/v1?q=secret",
        "https://example.com/v1#x",
        "ftp://localhost/v1",
    ] {
        assert!(OpenAiCompatible::new(url, "test".into(), "fixture").is_err());
    }
}

#[test]
fn bearer_header_rejects_control_characters_without_disclosing_key() {
    let err = OpenAiCompatible::new(
        "https://example.com/v1",
        "test".into(),
        "fixture\r\nInjected: secret",
    )
    .err()
    .unwrap()
    .to_string();
    assert!(!err.contains("fixture"));
    assert!(!err.contains("secret"));
}

#[test]
fn tool_failure_is_explicitly_source_data() {
    let encoded = encode_messages(&[
        Message::Assistant {
            content: "".into(),
            tool_calls: vec![ToolCall {
                id: "a".into(),
                name: "read_file".into(),
                arguments: json!({"path":"x"}),
            }],
        },
        Message::Tool {
            tool_call_id: "a".into(),
            content: "rejected".into(),
            is_error: true,
        },
    ]);
    let result: serde_json::Value =
        serde_json::from_str(encoded[1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(result["is_error"], true);
    assert_eq!(encoded[1]["role"], "tool");
}

#[test]
fn batched_tool_images_follow_all_function_outputs_without_instruction_promotion() {
    let image=namzu_core::ToolImage{width:2,height:2,png_base64:"iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEklEQVR4nGP4z8DAAMIM/4EAAB/uBfsL2WiLAAAAAElFTkSuQmCC".into()};
    assert!(image.png().is_ok());
    let messages = vec![
        Message::Assistant {
            content: String::new(),
            tool_calls: vec![
                ToolCall {
                    id: "a".into(),
                    name: "screen".into(),
                    arguments: json!({}),
                },
                ToolCall {
                    id: "b".into(),
                    name: "info".into(),
                    arguments: json!({}),
                },
            ],
        },
        Message::Tool {
            tool_call_id: "a".into(),
            content: "pixels".into(),
            is_error: false,
        },
        Message::ToolImage {
            tool_call_id: "a".into(),
            image: image.clone(),
        },
        Message::Tool {
            tool_call_id: "b".into(),
            content: "info".into(),
            is_error: false,
        },
    ];
    let wire = encode_messages(&messages);
    assert_eq!(wire[1]["role"], "tool");
    assert_eq!(wire[2]["role"], "tool");
    assert_eq!(wire[2]["tool_call_id"], "b");
    assert_eq!(wire[3]["role"], "user");
    assert_eq!(wire[3]["content"][1]["type"], "image_url");
    assert_eq!(wire[3]["content"][1]["image_url"]["url"], image.data_url());
    assert!(
        wire[3]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Untrusted screenshot from tool result a")
    );
}
