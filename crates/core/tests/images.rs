use base64::{Engine as _, engine::general_purpose::STANDARD};
use namzu_core::*;
use serde_json::json;
fn image() -> ToolImage {
    let mut bytes = Vec::new();
    {
        let mut e = png::Encoder::new(&mut bytes, 2, 2);
        e.set_color(png::ColorType::Rgb);
        e.set_depth(png::BitDepth::Eight);
        let mut w = e.write_header().unwrap();
        w.write_image_data(&[255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255])
            .unwrap();
    }
    ToolImage {
        width: 2,
        height: 2,
        png_base64: STANDARD.encode(bytes),
    }
}
fn call() -> ToolCall {
    ToolCall {
        id: "screen".into(),
        name: "screen".into(),
        arguments: json!({}),
    }
}
struct Screen {
    result: ToolResult,
}
impl Tools for Screen {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "screen".into(),
            description: "fixture".into(),
            parameters: json!({"type":"object"}),
        }]
    }
    fn call(&mut self, _: &ToolCall, _: &TurnContext) -> Result<ToolResult, AgentError> {
        Ok(self.result.clone())
    }
}
fn provider() -> ScriptedProvider {
    ScriptedProvider(
        vec![
            Completion {
                text: String::new(),
                tool_calls: vec![call()],
            },
            Completion {
                text: "done".into(),
                tool_calls: vec![],
            },
        ]
        .into(),
    )
}
#[test]
fn png_rejects_corruption_dimension_lies_and_oversize_before_decode() {
    let i = image();
    assert!(i.png().is_ok());
    assert!(!format!("{i:?}").contains(&i.png_base64));
    let mut wrong = i.clone();
    wrong.width = 3;
    assert!(wrong.png().is_err());
    let mut corrupt = i.clone();
    let mut b = corrupt.png().unwrap();
    b[40] ^= 1;
    corrupt.png_base64 = STANDARD.encode(b);
    assert!(corrupt.png().is_err());
    let mut large = i.clone();
    large.png_base64 = "A".repeat(IMAGE_BYTES * 2);
    assert!(large.png().is_err());
    let mut zero = i;
    zero.height = 0;
    assert!(zero.png().is_err());
}
#[test]
fn image_roundtrips_typed_history_and_is_revalidated_on_resume() {
    let mut s = Screen {
        result: ToolResult {
            content: "pixels".into(),
            is_error: false,
            image: Some(image()),
        },
    };
    let t = run_turn(
        &mut provider(),
        &mut s,
        "look",
        Limits::default(),
        Cancellation::default(),
    )
    .unwrap();
    assert!(
        matches!(&t.messages[3],Message::ToolImage{tool_call_id,image} if tool_call_id=="screen" && image.png().is_ok())
    );
    let history: Vec<Message> =
        serde_json::from_slice(&serde_json::to_vec(&t.messages).unwrap()).unwrap();
    let resumed = run_history(
        &mut ScriptedProvider(
            vec![Completion {
                text: "resumed".into(),
                tool_calls: vec![],
            }]
            .into(),
        ),
        &mut s,
        history,
        Limits::default(),
        Cancellation::default(),
        &mut NoopObserver,
    )
    .unwrap();
    assert_eq!(resumed.output, "resumed");
}
#[test]
fn invalid_and_error_images_never_reach_next_provider_call() {
    for bad in [false, true] {
        let mut i = image();
        if bad {
            i.width = 9;
        }
        let mut s = Screen {
            result: ToolResult {
                content: "bad".into(),
                is_error: !bad,
                image: Some(i),
            },
        };
        let mut p = provider();
        assert!(
            run_turn(
                &mut p,
                &mut s,
                "look",
                Limits::default(),
                Cancellation::default()
            )
            .is_err()
        );
        assert_eq!(p.0.len(), 1);
    }
}
#[test]
fn image_bytes_count_against_conversation_budget() {
    let mut s = Screen {
        result: ToolResult {
            content: "pixels".into(),
            is_error: false,
            image: Some(image()),
        },
    };
    let mut p = provider();
    assert!(
        run_turn(
            &mut p,
            &mut s,
            "look",
            Limits {
                max_text_bytes: 150,
                ..Limits::default()
            },
            Cancellation::default()
        )
        .is_err()
    );
    assert_eq!(p.0.len(), 1);
}
#[test]
fn orphan_duplicate_and_error_history_images_are_rejected_before_provider() {
    for history in [
        vec![Message::ToolImage {
            tool_call_id: "screen".into(),
            image: image(),
        }],
        vec![
            Message::Assistant {
                content: String::new(),
                tool_calls: vec![call()],
            },
            Message::Tool {
                tool_call_id: "screen".into(),
                content: String::new(),
                is_error: true,
            },
            Message::ToolImage {
                tool_call_id: "screen".into(),
                image: image(),
            },
        ],
        vec![
            Message::Assistant {
                content: String::new(),
                tool_calls: vec![call()],
            },
            Message::Tool {
                tool_call_id: "screen".into(),
                content: String::new(),
                is_error: false,
            },
            Message::ToolImage {
                tool_call_id: "screen".into(),
                image: image(),
            },
            Message::ToolImage {
                tool_call_id: "screen".into(),
                image: image(),
            },
        ],
    ] {
        let mut p = provider();
        let mut s = Screen {
            result: ToolResult::ok(""),
        };
        assert!(
            run_history(
                &mut p,
                &mut s,
                history,
                Limits::default(),
                Cancellation::default(),
                &mut NoopObserver
            )
            .is_err()
        );
        assert_eq!(p.0.len(), 2);
    }
}
