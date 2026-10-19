use namzu_core::*;
use namzu_tools::{FileTools, TerminalTools, ToolPolicy, content::*};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, time::Duration};
fn ctx() -> TurnContext {
    TurnContext::new(Duration::from_secs(30), Cancellation::default()).unwrap()
}
fn call(name: &str, args: Value) -> ToolCall {
    ToolCall {
        id: "content".into(),
        name: name.into(),
        arguments: args,
    }
}
fn png(
    color: png::ColorType,
    depth: png::BitDepth,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> Vec<u8> {
    let mut data = Vec::new();
    {
        let mut e = png::Encoder::new(&mut data, width, height);
        e.set_color(color);
        e.set_depth(depth);
        let mut w = e.write_header().unwrap();
        w.write_image_data(pixels).unwrap();
    }
    data
}
#[test]
fn utf8_pages_reassemble_large_file_and_identity_matches_exact_source() {
    let root = tempfile::tempdir().unwrap();
    let text = "Rust Türkçe 🌍\n".repeat(20000);
    fs::write(root.path().join("large"), &text).unwrap();
    let reader = ContentReader::new(root.path()).unwrap();
    let info = reader.inspect("large", &ctx()).unwrap();
    assert_eq!(
        info.sha256,
        format!("{:x}", Sha256::digest(text.as_bytes()))
    );
    assert_eq!(info.size_bytes, text.len());
    let mut got = String::new();
    let mut offset = 0;
    loop {
        let page = reader
            .read_page("large", offset, 65535, Some(&info.sha256), &ctx())
            .unwrap();
        assert_eq!(page.file, info);
        assert!(page.content.len() <= 65535);
        assert!(page.next_offset > offset);
        offset = page.next_offset;
        got.push_str(&page.content);
        if page.eof {
            break;
        }
    }
    assert_eq!(got, text);
    let end = reader
        .read_page("large", offset, 4, Some(&info.sha256), &ctx())
        .unwrap();
    assert!(end.eof && end.content.is_empty());
}
#[test]
fn byte_boundaries_and_changed_sources_fail_without_stale_page() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("x"), "a🌍b").unwrap();
    let r = ContentReader::new(root.path()).unwrap();
    let i = r.inspect("x", &ctx()).unwrap();
    let first = r.read_page("x", 0, 4, None, &ctx()).unwrap();
    assert_eq!(first.content, "a");
    assert_eq!(first.next_offset, 1);
    assert!(r.read_page("x", 2, 4, None, &ctx()).is_err());
    assert!(r.read_page("x", 7, 4, None, &ctx()).is_err());
    assert!(r.read_page("x", 0, 3, None, &ctx()).is_err());
    fs::write(root.path().join("x"), "changed").unwrap();
    assert!(r.read_page("x", 0, 4, Some(&i.sha256), &ctx()).is_err());
}
#[test]
fn source_budgets_binary_and_cancelled_reads_are_bounded() {
    let root = tempfile::tempdir().unwrap();
    let file = fs::File::create(root.path().join("huge")).unwrap();
    file.set_len(SOURCE_BYTES as u64 + 1).unwrap();
    fs::write(root.path().join("binary"), [0, 255, 1]).unwrap();
    let r = ContentReader::new(root.path()).unwrap();
    assert!(r.inspect("huge", &ctx()).is_err());
    assert_eq!(
        r.inspect("binary", &ctx()).unwrap().media_type,
        "application/octet-stream"
    );
    assert!(r.read_page("binary", 0, 4, None, &ctx()).is_err());
    let c = ctx();
    c.cancellation().cancel();
    assert_eq!(r.inspect("binary", &c).unwrap_err(), AgentError::Cancelled);
}
#[test]
fn json_pointer_paging_and_scalar_objects_preserve_data_types() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("data"),
        r#"{"a/b":{"~items":[1,true,null,{"message":"Türkçe ✓"}]},"object":{"x":2}}"#,
    )
    .unwrap();
    let r = ContentReader::new(root.path()).unwrap();
    let p = r
        .document(
            "data",
            DocumentFormat::Json,
            "/a~1b/~0items",
            1,
            2,
            None,
            &ctx(),
        )
        .unwrap();
    assert_eq!(p.value, json!([true, null]));
    assert_eq!((p.total, p.next_start, p.eof), (4, 3, false));
    let p = r
        .document(
            "data",
            DocumentFormat::Json,
            "/a~1b/~0items",
            3,
            2,
            Some(&p.file.sha256),
            &ctx(),
        )
        .unwrap();
    assert_eq!(p.value, json!([{"message":"Türkçe ✓"}]));
    assert!(p.eof);
    assert_eq!(
        r.document("data", DocumentFormat::Json, "/object", 0, 1, None, &ctx())
            .unwrap()
            .value,
        json!({"x":2})
    );
    for pointer in ["missing", "/a~2b", "/missing"] {
        assert!(
            r.document("data", DocumentFormat::Json, pointer, 0, 1, None, &ctx())
                .is_err()
        );
    }
    assert!(
        r.document("data", DocumentFormat::Json, "/object", 1, 1, None, &ctx())
            .is_err()
    );
}
#[test]
fn jsonl_validates_unselected_tail_and_preserves_physical_line_numbers() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("log"), "\r\n{\"x\":1}\r\n \ntrue\nnull\n").unwrap();
    let r = ContentReader::new(root.path()).unwrap();
    let p = r
        .document("log", DocumentFormat::Jsonl, "", 1, 1, None, &ctx())
        .unwrap();
    assert_eq!(p.total, 3);
    assert_eq!(p.value, json!([{"line":4,"value":true}]));
    fs::write(root.path().join("log"), "1\n2\ninvalid\n").unwrap();
    assert!(
        r.document("log", DocumentFormat::Jsonl, "", 0, 1, None, &ctx())
            .is_err()
    );
    fs::write(root.path().join("log"), "\n \r\n").unwrap();
    let p = r
        .document("log", DocumentFormat::Jsonl, "", 0, 1, None, &ctx())
        .unwrap();
    assert_eq!(p.total, 0);
    assert!(p.eof);
}
#[test]
fn structured_source_depth_record_and_selected_output_limits() {
    let root = tempfile::tempdir().unwrap();
    let r = ContentReader::new(root.path()).unwrap();
    for source in [
        "[".repeat(200) + &"]".repeat(200),
        json!({"large":"x".repeat(65536)}).to_string(),
        "null trailing".into(),
    ] {
        fs::write(root.path().join("x"), source).unwrap();
        assert!(
            r.document("x", DocumentFormat::Json, "", 0, 1, None, &ctx())
                .is_err()
        );
    }
    fs::write(root.path().join("x"), "0\n".repeat(16385)).unwrap();
    assert!(
        r.document("x", DocumentFormat::Jsonl, "", 0, 1, None, &ctx())
            .is_err()
    );
    let file = fs::File::create(root.path().join("x")).unwrap();
    file.set_len(DOCUMENT_BYTES as u64 + 1).unwrap();
    assert!(
        r.document("x", DocumentFormat::Json, "", 0, 1, None, &ctx())
            .is_err()
    );
}
#[test]
fn png_rgb_gray_and_alpha_attach_real_pixels_on_white() {
    let root = tempfile::tempdir().unwrap();
    let r = ContentReader::new(root.path()).unwrap();
    for (color, pixels, expected) in [
        (
            png::ColorType::Rgba,
            vec![255, 0, 0, 128, 0, 0, 255, 0],
            vec![255, 127, 127, 255, 255, 255],
        ),
        (
            png::ColorType::GrayscaleAlpha,
            vec![0, 255, 10, 0],
            vec![0, 0, 0, 255, 255, 255],
        ),
        (
            png::ColorType::Rgb,
            vec![1, 2, 3, 4, 5, 6],
            vec![1, 2, 3, 4, 5, 6],
        ),
    ] {
        let bytes = png(color, png::BitDepth::Eight, 2, 1, &pixels);
        fs::write(root.path().join("image.bin"), &bytes).unwrap();
        let a = r.image("image.bin", None, &ctx()).unwrap();
        assert_eq!(a.file.media_type, "image/png");
        assert_eq!((a.image.width, a.image.height), (2, 1));
        let encoded = a.image.png().unwrap();
        let mut decoder = png::Decoder::new(encoded.as_slice()).read_info().unwrap();
        let mut actual = vec![0; decoder.output_buffer_size()];
        decoder.next_frame(&mut actual).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(a.file.sha256, format!("{:x}", Sha256::digest(&bytes)));
    }
}
#[test]
fn corrupt_oversize_16bit_and_non_png_images_do_not_become_model_content() {
    let root = tempfile::tempdir().unwrap();
    let r = ContentReader::new(root.path()).unwrap();
    let mut corrupt = png(png::ColorType::Rgb, png::BitDepth::Eight, 1, 1, &[1, 2, 3]);
    corrupt[40] ^= 1;
    for data in [
        b"not PNG".to_vec(),
        corrupt,
        png(
            png::ColorType::Grayscale,
            png::BitDepth::Sixteen,
            1,
            1,
            &[0, 255],
        ),
        png(
            png::ColorType::Grayscale,
            png::BitDepth::Eight,
            1921,
            1,
            &vec![0; 1921],
        ),
    ] {
        fs::write(root.path().join("x"), data).unwrap();
        assert!(r.image("x", None, &ctx()).is_err());
    }
    fs::write(root.path().join("x"), vec![0; IMAGE_BYTES + 1]).unwrap();
    assert!(r.image("x", None, &ctx()).is_err());
}
#[test]
fn workspace_paths_symlinks_and_special_files_cannot_escape_or_block() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret"), "private").unwrap();
    let r = ContentReader::new(root.path()).unwrap();
    for path in ["../secret", "/etc/passwd", "", "x/../../secret"] {
        assert!(r.inspect(path, &ctx()).is_err());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink(outside.path().join("secret"), root.path().join("link")).unwrap();
        assert!(r.inspect("link", &ctx()).is_err());
        symlink(outside.path(), root.path().join("directory")).unwrap();
        assert!(r.inspect("directory/secret", &ctx()).is_err());
        let status = std::process::Command::new("mkfifo")
            .arg(root.path().join("fifo"))
            .status()
            .unwrap();
        assert!(status.success());
        assert!(r.inspect("fifo", &ctx()).is_err());
    }
    assert!(r.inspect(".", &ctx()).is_err());
}
#[test]
fn tool_policy_and_typed_image_history_use_the_same_sdk_path() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("x"),
        png(
            png::ColorType::Rgb,
            png::BitDepth::Eight,
            1,
            1,
            &[20, 40, 60],
        ),
    )
    .unwrap();
    let mut tools = TerminalTools::new(root.path(), ToolPolicy::default()).unwrap();
    assert!(
        CONTENT_NAMES
            .iter()
            .all(|n| tools.definitions().iter().any(|d| d.name == *n))
    );
    let mut provider = ScriptedProvider(
        vec![
            Completion {
                tool_calls: vec![call("read_image", json!({"path":"x"}))],
                ..Default::default()
            },
            Completion {
                text: "done".into(),
                ..Default::default()
            },
        ]
        .into(),
    );
    let turn = run_turn(
        &mut provider,
        &mut tools,
        "look",
        Limits::default(),
        Cancellation::default(),
    )
    .unwrap();
    assert!(
        turn.messages
            .iter()
            .any(|m| matches!(m,Message::ToolImage{image,..} if image.width==1))
    );
    let mut tools = TerminalTools::new(
        root.path(),
        ToolPolicy {
            deny: ["read_image".into()].into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!tools.definitions().iter().any(|d| d.name == "read_image"));
    assert!(
        tools
            .call(&call("read_image", json!({"path":"x"})), &ctx())
            .unwrap()
            .is_error
    );
}
#[test]
fn malformed_content_batch_is_rejected_before_prior_write() {
    let root = tempfile::tempdir().unwrap();
    let mut tools = FileTools::new(root.path(), true).unwrap();
    let mut provider = ScriptedProvider(
        vec![Completion {
            tool_calls: vec![
                call("write_file", json!({"path":"effect","content":"unsafe"})),
                ToolCall {
                    id: "later".into(),
                    name: "read_document".into(),
                    arguments: json!({"path":"x","format":"json","pointer":"~bad"}),
                },
            ],
            ..Default::default()
        }]
        .into(),
    );
    assert!(
        run_turn(
            &mut provider,
            &mut tools,
            "test",
            Limits::default(),
            Cancellation::default()
        )
        .is_err()
    );
    assert!(!root.path().join("effect").exists());
}
