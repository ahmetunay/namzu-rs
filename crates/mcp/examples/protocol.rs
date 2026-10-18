//! Offline transport-free peer transcript. No subprocess, HTTP, model or account.
use namzu_core::{Cancellation, TurnContext};
use namzu_mcp::{ClientSession, Incoming, Limits, Message, types::Implementation};
use serde_json::json;
use std::time::Duration;

fn request_id(message: &Message) -> namzu_mcp::Id {
    match message {
        Message::Request { id, .. } => id.clone(),
        _ => panic!("expected request"),
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let context = TurnContext::new(Duration::from_secs(300), Cancellation::default())?;
    let mut client =
        ClientSession::new(Implementation::new("namzu-rs", "0.1.0"), Limits::default())?;
    let initialize = client.initialize(&context)?;
    client.encode(&initialize)?;
    let response = json!({"jsonrpc":"2.0","id":request_id(&initialize),"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"offline-fixture","version":"1"}}});
    assert!(matches!(
        client.receive(&serde_json::to_vec(&response)?)?,
        Incoming::Initialized(_)
    ));
    client.encode(&client.initialized_notification()?)?;
    client.mark_initialized_sent()?;
    let call = client.request(
        "tools/call",
        Some(
            json!({"name":"fixture-answer","arguments":{}})
                .as_object()
                .unwrap()
                .clone(),
        ),
        &context,
    )?;
    let response = json!({"jsonrpc":"2.0","id":request_id(&call),"result":{"content":[{"type":"text","text":"12"}],"isError":false}});
    let Incoming::Reply {
        result: Ok(result), ..
    } = client.receive(&serde_json::to_vec(&response)?)?
    else {
        panic!("expected result")
    };
    assert_eq!(result["content"][0]["text"], "12");
    let waiting = client.request("ping", None, &context)?;
    let cancellation = client.cancel(&request_id(&waiting))?.expect("pending");
    client.encode(&cancellation)?;
    let late = json!({"jsonrpc":"2.0","id":request_id(&waiting),"result":{}});
    assert_eq!(
        client.receive(&serde_json::to_vec(&late)?)?,
        Incoming::IgnoredLateReply
    );
    assert!(client.close().is_empty());
    println!(
        "{}",
        json!({"offline":true,"protocol_version":"2025-11-25","fixture_answer":"12","late_reply_ignored":true,"model_calls":0,"real_server":false})
    );
    Ok(())
}
