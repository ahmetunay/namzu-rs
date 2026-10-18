//! Offline native peer for owned-computer acceptance; never a production server.
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
fn main() {
    for line in io::stdin().lock().lines() {
        let request: Value = serde_json::from_str(&line.unwrap()).unwrap();
        let Some(id) = request.get("id") else {
            continue;
        };
        let result = match request["method"].as_str().unwrap() {
            "initialize" => {
                json!({"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"native-guest-fixture","version":"1"}})
            }
            "tools/list" => {
                json!({"tools":[{"name":"write","inputSchema":{"type":"object","properties":{},"additionalProperties":false},"outputSchema":{"type":"object","properties":{"answer":{"type":"integer"}},"required":["answer"]}}]})
            }
            "tools/call" => {
                std::fs::write("mcp-result", b"42\n").unwrap();
                json!({"content":[{"type":"text","text":"owned guest"}],"structuredContent":{"answer":42,"guest_env":std::env::var("DOCKER_HOST").unwrap_or_default()}})
            }
            _ => panic!("unexpected method"),
        };
        println!("{}", json!({"jsonrpc":"2.0","id":id,"result":result}));
        io::stdout().flush().unwrap();
    }
}
