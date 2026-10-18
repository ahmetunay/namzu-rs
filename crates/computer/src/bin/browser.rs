//! Bounded guest Chromium worker; accepts no arbitrary JS or CDP methods.
use namzu_computer::browser::{
    AxNode, DomNode, MAX_NODES, Page, Reply, Request, TEXT_BUDGET,
    cdp::{Cdp, Result},
};
use serde_json::{Value, json};
use std::io::{Read, Write};
fn frame(c: &mut Cdp) -> Result<Value> {
    Ok(c.call("Page.getFrameTree", json!({}))?["frameTree"]["frame"].clone())
}
fn frame_url(frame: &Value) -> Result<String> {
    let url = frame["url"].as_str().ok_or("frame URL")?;
    let fragment = frame
        .get("urlFragment")
        .and_then(Value::as_str)
        .unwrap_or("");
    if !fragment.is_empty() && (!fragment.starts_with('#') || url.contains('#')) {
        return Err("invalid frame fragment".into());
    }
    Ok(format!("{url}{fragment}"))
}
fn page(c: &mut Cdp) -> Result<Page> {
    let f = frame(c)?;
    let h = c.call("Page.getNavigationHistory", json!({}))?;
    let i = h["currentIndex"].as_u64().ok_or("history index")? as usize;
    let entry = h["entries"]
        .as_array()
        .and_then(|v| v.get(i))
        .ok_or("history entry")?;
    let out = Page {
        url: frame_url(&f)?,
        title: entry["title"].as_str().unwrap_or("").into(),
        frame_id: f["id"].as_str().ok_or("frame id")?.into(),
        loader_id: f["loaderId"].as_str().ok_or("loader id")?.into(),
    };
    let after = frame(c)?;
    if entry["url"] != out.url
        || after["loaderId"] != out.loader_id
        || frame_url(&after)? != out.url
    {
        return Err("page changed during inspection".into());
    }
    Ok(out)
}
struct Budget {
    left: usize,
    truncated: bool,
}
impl Budget {
    fn text(&mut self, s: &str, limit: usize) -> String {
        let mut end = s.len().min(limit).min(self.left);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        if end < s.len() {
            self.truncated = true;
        }
        self.left -= end;
        s[..end].into()
    }
}
fn snapshot(c: &mut Cdp) -> Result<Reply> {
    let page = page(c)?;
    // Validate metadata before subtracting from the shared snapshot text budget.
    Reply::Info {
        page: Page {
            url: page.url.clone(),
            title: page.title.clone(),
            frame_id: page.frame_id.clone(),
            loader_id: page.loader_id.clone(),
        },
    }
    .validate()?;
    let root = c.call("DOM.getDocument", json!({"depth":-1,"pierce":false}))?;
    let tree = c.call(
        "Accessibility.getFullAXTree",
        json!({"frameId":page.frame_id}),
    )?;
    let after = frame(c)?;
    if after["loaderId"] != page.loader_id || frame_url(&after)? != page.url {
        return Err("page changed during snapshot".into());
    }
    let mut b = Budget {
        left: TEXT_BUDGET
            - page.url.len()
            - page.title.len()
            - page.frame_id.len()
            - page.loader_id.len(),
        truncated: false,
    };
    let mut dom = Vec::new();
    let mut stack = vec![(&root["root"], None)];
    while let Some((n, parent_id)) = stack.pop() {
        if dom.len() == MAX_NODES || b.left < 128 {
            b.truncated = true;
            break;
        }
        let node_id = n["nodeId"].as_i64().ok_or("DOM id")?;
        let attrs = n["attributes"].as_array().map(Vec::as_slice).unwrap_or(&[]);
        if attrs.len() > 32 {
            b.truncated = true;
        }
        let attributes = attrs
            .as_chunks::<2>()
            .0
            .iter()
            .take(16)
            .map(|p| {
                (
                    b.text(p[0].as_str().unwrap_or(""), 256),
                    b.text(p[1].as_str().unwrap_or(""), 512),
                )
            })
            .collect();
        dom.push(DomNode {
            node_id,
            backend_node_id: n["backendNodeId"].as_i64().ok_or("backend id")?,
            parent_id,
            node_type: n["nodeType"].as_u64().ok_or("node type")? as u16,
            name: b.text(n["nodeName"].as_str().unwrap_or(""), 256),
            text: b.text(n["nodeValue"].as_str().unwrap_or(""), 1024),
            attributes,
        });
        if let Some(children) = n["children"].as_array() {
            for child in children.iter().rev() {
                stack.push((child, Some(node_id)));
            }
        }
    }
    let mut accessibility = Vec::new();
    let nodes = tree["nodes"].as_array().ok_or("AX nodes")?;
    for n in nodes {
        if accessibility.len() == MAX_NODES || b.left < 128 {
            b.truncated = true;
            break;
        }
        let ids = n["childIds"].as_array().map(Vec::as_slice).unwrap_or(&[]);
        if ids.len() > MAX_NODES {
            b.truncated = true;
        }
        accessibility.push(AxNode {
            node_id: b.text(n["nodeId"].as_str().ok_or("AX id")?, 128),
            backend_node_id: n["backendDOMNodeId"].as_i64(),
            role: b.text(n["role"]["value"].as_str().unwrap_or(""), 256),
            name: b.text(n["name"]["value"].as_str().unwrap_or(""), 1024),
            value: b.text(n["value"]["value"].as_str().unwrap_or(""), 1024),
            ignored: n["ignored"].as_bool().unwrap_or(false),
            children: ids
                .iter()
                .take(MAX_NODES)
                .map(|v| b.text(v.as_str().unwrap_or(""), 128))
                .collect(),
        });
    }
    Ok(Reply::Snapshot {
        page,
        dom,
        accessibility,
        truncated: b.truncated,
    })
}
fn run(request: Request) -> Result<Reply> {
    request.validate()?;
    let mut c = Cdp::connect()?;
    let reply = match request {
        Request::Info => Reply::Info {
            page: page(&mut c)?,
        },
        Request::Snapshot => snapshot(&mut c)?,
        Request::Navigate { url } => {
            c.call("Page.enable", json!({}))?;
            c.call("Page.setLifecycleEventsEnabled", json!({"enabled":true}))?;
            c.clear_events();
            let result = c.call("Page.navigate", json!({"url":url}))?;
            if result.get("errorText").is_some() || result["isDownload"] == true {
                return Err("navigation rejected or became download".into());
            }
            if let Some(loader) = result["loaderId"].as_str() {
                c.wait_loaded(loader)?;
            }
            Reply::Navigated {
                page: page(&mut c)?,
            }
        }
    };
    reply.validate()?;
    Ok(reply)
}
fn main() {
    let result = (|| -> Result<Reply> {
        let mut bytes = Vec::new();
        std::io::stdin().take(16385).read_to_end(&mut bytes)?;
        if bytes.len() > 16384 {
            return Err("input limit".into());
        }
        run(serde_json::from_slice(&bytes)?)
    })();
    let safe: std::result::Result<Reply, &str> =
        result.map_err(|_| "guest browser unavailable or inspection failed");
    if let Ok(bytes) = serde_json::to_vec(&safe)
        && bytes.len() <= namzu_computer::protocol::WIRE_LIMIT
    {
        let _ = std::io::stdout().write_all(&bytes);
        return;
    }
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frame_metadata_reassembles_fragments_without_losing_the_real_url() {
        assert_eq!(
            frame_url(&json!({"url":"https://example.com/page","urlFragment":"#section"})).unwrap(),
            "https://example.com/page#section"
        );
        assert_eq!(
            frame_url(&json!({"url":"about:blank"})).unwrap(),
            "about:blank"
        );
        assert!(frame_url(&json!({"url":"https://example.com/","urlFragment":"bad"})).is_err());
    }
    #[test]
    fn snapshot_text_budget_truncates_on_utf8_boundaries() {
        let mut budget = Budget {
            left: 5,
            truncated: false,
        };
        assert_eq!(budget.text("üüü", 1024), "üü");
        assert_eq!(budget.left, 1);
        assert!(budget.truncated);
        assert_eq!(budget.text("ü", 1024), "");
        assert_eq!(budget.text("x", 1024), "x");
        assert_eq!(budget.left, 0);
    }
    #[test]
    fn per_field_limit_does_not_consume_the_rest_of_the_budget() {
        let mut budget = Budget {
            left: 4096,
            truncated: false,
        };
        assert_eq!(budget.text(&"x".repeat(2048), 1024).len(), 1024);
        assert_eq!(budget.left, 3072);
        assert!(budget.truncated);
        assert_eq!(budget.text("next", 1024), "next");
    }
}
