use namzu_mcp::{
    discovery::{Error, Item, Kind, Pager, prompt_result, read_result, tool_result},
    schema,
};
use serde_json::{Map, Value, json};
fn map(value: Value) -> Map<String, Value> {
    value.as_object().unwrap().clone()
}
fn tool(name: &str) -> Value {
    json!({"name":name,"inputSchema":{"type":"object","properties":{"value":{"type":"integer"}}}})
}
fn catalog(kind: Kind, page: Value) -> Result<namzu_mcp::discovery::Catalog, Error> {
    let mut pager = Pager::new(kind);
    pager.push(map(page))?;
    pager.finish("fixture".into(), 1)
}
#[test]
fn valid_tools_schema_extensions_and_opaque_cursor_roundtrip() {
    let mut pager = Pager::new(Kind::Tools);
    assert_eq!(pager.request_params().unwrap(), None);
    assert!(
        !pager
            .push(map(
                json!({"tools":[tool("read")],"nextCursor":"opaque /+?=✓"})
            ))
            .unwrap()
    );
    assert_eq!(
        pager.request_params().unwrap().unwrap()["cursor"],
        "opaque /+?=✓"
    );
    assert!(
        pager
            .push(map(json!({"tools":[tool("write")],"extension":42})))
            .unwrap()
    );
    let result = pager.finish("one".into(), 2).unwrap();
    assert_eq!(result.items.len(), 2);
}
#[test]
fn empty_catalog_is_valid_but_unfinished_builder_is_not() {
    assert!(Pager::new(Kind::Tools).finish("one".into(), 1).is_err());
    assert!(
        catalog(Kind::Tools, json!({"tools":[]}))
            .unwrap()
            .items
            .is_empty()
    );
}
#[test]
fn failed_page_poison_prevents_publishing_a_valid_prefix() {
    let mut pager = Pager::new(Kind::Tools);
    assert!(
        pager
            .push(map(json!({"tools":[tool("valid"),tool("bad name")]})))
            .is_err()
    );
    assert!(pager.finish("one".into(), 1).is_err());
}
#[test]
fn duplicate_items_and_cursor_cycles_are_rejected() {
    let mut pager = Pager::new(Kind::Tools);
    pager
        .push(map(json!({"tools":[tool("read")],"nextCursor":"a"})))
        .unwrap();
    assert_eq!(
        pager.push(map(json!({"tools":[tool("read")]}))),
        Err(Error::Duplicate)
    );
    let mut pager = Pager::new(Kind::Tools);
    pager
        .push(map(json!({"tools":[],"nextCursor":"a"})))
        .unwrap();
    pager
        .push(map(json!({"tools":[],"nextCursor":"b"})))
        .unwrap();
    assert_eq!(
        pager.push(map(json!({"tools":[],"nextCursor":"a"}))),
        Err(Error::Cursor)
    );
}
#[test]
fn malformed_empty_null_or_excessive_cursors_are_not_end_markers() {
    for cursor in [
        json!(null),
        json!(""),
        json!(42),
        json!("\n"),
        json!("x".repeat(1025)),
    ] {
        assert_eq!(
            catalog(Kind::Tools, json!({"tools":[],"nextCursor":cursor})).unwrap_err(),
            Error::Cursor
        );
    }
}
#[test]
fn malformed_pages_items_and_names_fail() {
    for value in [
        json!({}),
        json!({"tools":{}}),
        json!({"tools":[null]}),
        json!({"tools":[tool("")]}),
        json!({"tools":[tool("a/b")]}),
        json!({"tools":[tool(&"a".repeat(129))]}),
    ] {
        assert!(catalog(Kind::Tools, value).is_err());
    }
    let mut item = tool("okay");
    item["description"] = Value::Null;
    assert!(catalog(Kind::Tools, json!({"tools":[item]})).is_err());
}
#[test]
fn aggregate_item_page_and_byte_budgets_are_enforced() {
    assert_eq!(
        catalog(
            Kind::Tools,
            json!({"tools":(0..1025).map(|n|tool(&format!("t{n}"))).collect::<Vec<_>>()})
        )
        .unwrap_err(),
        Error::Limit
    );
    let mut pager = Pager::new(Kind::Tools);
    for n in 0..32 {
        pager.request_params().unwrap();
        pager
            .push(map(json!({"tools":[],"nextCursor":format!("c{n}")})))
            .unwrap();
    }
    assert_eq!(pager.request_params(), Err(Error::Limit));
    assert!(pager.finish("one".into(), 1).is_err());
    let mut pager = Pager::new(Kind::Tools);
    let mut exceeded = false;
    for n in 0..12 {
        if pager
            .push(map(
                json!({"tools":[],"nextCursor":format!("c{n}"),"extension":"x".repeat(400000)}),
            ))
            .is_err()
        {
            exceeded = true;
            break;
        }
    }
    assert!(exceeded);
}
#[test]
fn valid_resources_prompts_and_templates_keep_extensions() {
    assert!(
        matches!(catalog(Kind::Resources,json!({"resources":[{"uri":"mem://one","name":"Örnek","extra":42}]})).unwrap().items[0],Item::Resource(ref r) if r.extra["extra"]==42)
    );
    assert!(
        catalog(
            Kind::Prompts,
            json!({"prompts":[{"name":"review","arguments":[{"name":"path","required":true}]}]})
        )
        .is_ok()
    );
    assert!(
        catalog(
            Kind::Templates,
            json!({"resourceTemplates":[{"name":"Files","uriTemplate":"file:///{+path}{?query}"}]})
        )
        .is_ok()
    );
}
#[test]
fn invalid_uri_template_and_duplicate_prompt_arguments_fail() {
    for uri in [
        "",
        ":empty",
        "file://has space",
        "relative/path",
        "file:///{bad",
        "file:///{}",
    ] {
        assert!(
            catalog(
                Kind::Templates,
                json!({"resourceTemplates":[{"name":"Files","uriTemplate":uri}]})
            )
            .is_err()
        );
    }
    assert_eq!(
        catalog(
            Kind::Prompts,
            json!({"prompts":[{"name":"review","arguments":[{"name":"x"},{"name":"x"}]}]})
        )
        .unwrap_err(),
        Error::Duplicate
    );
}
#[test]
fn schema_validates_shapes_bounds_and_local_refs_without_expansion() {
    assert!(schema::check(&map(json!({"type":"object","$defs":{"Node":{"type":"object","properties":{"child":{"$ref":"#/$defs/Node"}}}},"properties":{"root":{"$ref":"#/$defs/Node"}},"required":["root"]}))).is_ok());
    for schema in [
        json!({"type":"array"}),
        json!({"type":"object","required":"bad"}),
        json!({"type":"object","required":["a","a"]}),
        json!({"type":"object","properties":{"x":42}}),
        json!({"type":"object","$ref":"#/$defs/missing"}),
        json!({"type":"object","$ref":"https://DO_NOT_FETCH/schema"}),
        json!({"type":"object","multipleOf":0}),
        json!({"type":"object","oneOf":[]}),
        json!({"type":"object","properties":{"x":{"type":"unknown"}}}),
    ] {
        assert_eq!(schema::check(&map(schema)), Err(Error::Schema));
    }
    assert_eq!(
        schema::check(&map(
            json!({"type":"object","description":"x".repeat(65537)})
        )),
        Err(Error::Schema)
    );
}
#[test]
fn schemas_reject_deep_or_expansive_host_built_metadata_before_serde() {
    let mut value = json!({"type":"object"});
    for _ in 0..50 {
        value = json!({"type":"object","properties":{"nested":value}});
    }
    assert_eq!(schema::check(&map(value)), Err(Error::Schema));
}
#[test]
fn resource_content_requires_exactly_one_text_or_blob_and_valid_uri() {
    assert!(read_result(map(json!({"contents":[{"uri":"mem://text","text":"hello"},{"uri":"mem://binary","blob":"AA==","mimeType":"application/octet-stream"}]}))).is_ok());
    for content in [
        json!({"uri":"mem://x"}),
        json!({"uri":"mem://x","text":"a","blob":"AA=="}),
        json!({"uri":"relative","text":"a"}),
        json!({"uri":"mem://x","text":null}),
        json!({"uri":"mem://x","text":"a","mimeType":null}),
    ] {
        assert!(read_result(map(json!({"contents":[content]}))).is_err());
    }
}
#[test]
fn prompt_and_tool_content_are_typed_and_never_open_resource_uris() {
    assert!(prompt_result(map(json!({"messages":[{"role":"user","content":{"type":"resource_link","uri":"file:///DO_NOT_OPEN","name":"link"}}]}))).is_ok());
    assert!(
        prompt_result(map(
            json!({"messages":[{"role":"system","content":{"type":"text","text":"untrusted"}}]})
        ))
        .is_err()
    );
    assert!(tool_result(map(json!({"content":[{"type":"resource","resource":{"uri":"mem://one","text":"hello"}},{"type":"image","data":"opaque","mimeType":"image/png"}],"isError":true}))).unwrap().is_error);
    assert!(tool_result(map(json!({"content":[{"type":"unknown","text":"bad"}]}))).is_err());
}
