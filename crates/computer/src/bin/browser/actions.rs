//! Fixed DOM operations in an isolated world; no caller-provided JS or selectors.
use super::{Cdp, Request, Result, json, page};
use namzu_computer::browser::{Document, Reply, Target, press_key};
use serde_json::Value;
pub fn document(c: &mut Cdp, expected: &Document) -> Result<()> {
    if Document::from(&page(c)?) != *expected {
        return Err("stale document".into());
    }
    Ok(())
}
fn target(c: &mut Cdp, t: &Target) -> Result<String> {
    document(c, &t.document)?;
    let tree = c.call("DOM.getDocument", json!({"depth":-1,"pierce":false}))?;
    let mut stack = vec![&tree["root"]];
    let mut found = false;
    while let Some(n) = stack.pop() {
        if n["backendNodeId"] == t.backend_node_id {
            found = n["nodeType"] == 1;
            break;
        }
        if let Some(children) = n["children"].as_array() {
            stack.extend(children);
        }
    }
    if !found {
        return Err("target not in current main document".into());
    }
    let world = c.call(
        "Page.createIsolatedWorld",
        json!({"frameId":t.document.frame_id,"worldName":"namzu-browser-actions"}),
    )?;
    let object=c.call("DOM.resolveNode",json!({"backendNodeId":t.backend_node_id,"executionContextId":world["executionContextId"],"objectGroup":"namzu-action"}))?;
    let id = object["object"]["objectId"]
        .as_str()
        .ok_or("target object missing")?
        .to_owned();
    document(c, &t.document)?;
    Ok(id)
}
fn js(c: &mut Cdp, id: &str, function: &str, args: Vec<Value>) -> Result<()> {
    let result=c.call("Runtime.callFunctionOn",json!({"objectId":id,"functionDeclaration":function,"arguments":args.into_iter().map(|v|json!({"value":v})).collect::<Vec<_>>(),"returnByValue":true,"awaitPromise":false}))?;
    if result.get("exceptionDetails").is_some() || result["result"]["value"] != true {
        return Err("target action rejected".into());
    }
    Ok(())
}
const FILL: &str = r#"function(value) {
 if (!this.isConnected || this.matches(':disabled') || this.readOnly) return false;
 let proto;
 if (this instanceof HTMLTextAreaElement) proto=HTMLTextAreaElement.prototype;
 else if (this instanceof HTMLInputElement && ['text','search','email','tel','url','password'].includes(this.type)) proto=HTMLInputElement.prototype;
 else return false;
 Object.getOwnPropertyDescriptor(proto,'value').set.call(this,value);
 this.dispatchEvent(new Event('input',{bubbles:true}));
 this.dispatchEvent(new Event('change',{bubbles:true}));return true;
}"#;
const SELECT: &str = r#"function(values) {
 if (!this.isConnected || !(this instanceof HTMLSelectElement) || this.matches(':disabled') || (!this.multiple && values.length!==1)) return false;
 const options=Array.from(this.options);
 if (values.some(v=>!options.some(o=>o.value===v && !o.disabled && !(o.parentElement instanceof HTMLOptGroupElement && o.parentElement.disabled)))) return false;
 for (const option of options) option.selected=values.includes(option.value);
 this.dispatchEvent(new Event('input',{bubbles:true}));this.dispatchEvent(new Event('change',{bubbles:true}));return true;
}"#;
const INTERACTIVE: &str = r#"function() {return this.isConnected && !this.matches(':disabled') && !(this instanceof HTMLInputElement && this.type==='file');}"#;
const UPLOAD: &str = r#"function() {return this.isConnected && this instanceof HTMLInputElement && this.type==='file' && !this.matches(':disabled');}"#;
pub fn apply(c: &mut Cdp, request: Request) -> Result<Reply> {
    let t = match &request {
        Request::Click { target }
        | Request::Fill { target, .. }
        | Request::Select { target, .. }
        | Request::Press { target, .. }
        | Request::ScrollIntoView { target }
        | Request::Upload { target, .. } => target,
        _ => return Err("action required".into()),
    };
    let id = target(c, t)?;
    match &request {
        Request::Fill { text, .. } => js(c, &id, FILL, vec![json!(text)])?,
        Request::Select { values, .. } => js(c, &id, SELECT, vec![json!(values)])?,
        Request::Upload { path, .. } => {
            js(c, &id, UPLOAD, vec![])?;
            let staged = super::transfers::stage_upload(path)?;
            let file = staged
                .path()
                .join(std::path::Path::new(path).file_name().ok_or("file name")?);
            document(c, &t.document)?;
            c.call(
                "DOM.setFileInputFiles",
                json!({"objectId":id,"files":[file]}),
            )?;
            // Chromium reads the file lazily when a later form submission occurs.
            let _ = staged.keep();
        }
        Request::ScrollIntoView { .. } => {
            c.call("DOM.scrollIntoViewIfNeeded", json!({"objectId":id}))?;
        }
        Request::Press { key, .. } => {
            js(c, &id, INTERACTIVE, vec![])?;
            c.call("DOM.focus", json!({"objectId":id}))?;
            let (key, code) = press_key(key).ok_or("unsupported key")?;
            let text = if key == "Enter" { "\r" } else { "" };
            document(c, &t.document)?;
            c.call("Input.dispatchKeyEvent",json!({"type":"keyDown","key":key,"windowsVirtualKeyCode":code,"nativeVirtualKeyCode":code,"text":text}))?;
            c.call("Input.dispatchKeyEvent",json!({"type":"keyUp","key":key,"windowsVirtualKeyCode":code,"nativeVirtualKeyCode":code}))?;
        }
        Request::Click { .. } => {
            js(c, &id, INTERACTIVE, vec![])?;
            c.call("DOM.scrollIntoViewIfNeeded", json!({"objectId":id}))?;
            let box_model = c.call("DOM.getBoxModel", json!({"objectId":id}))?;
            let quad = box_model["model"]["content"]
                .as_array()
                .ok_or("visible box required")?;
            if quad.len() != 8 {
                return Err("invalid box".into());
            }
            let values = quad
                .iter()
                .map(|v| {
                    v.as_f64()
                        .filter(|n| n.is_finite())
                        .ok_or("finite box required")
                })
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let x = (values[0] + values[2] + values[4] + values[6]) / 4.0;
            let y = (values[1] + values[3] + values[5] + values[7]) / 4.0;
            js(
                c,
                &id,
                r#"function(x,y) {const hit=this.ownerDocument.elementFromPoint(x,y);return this.isConnected && hit!==null && (hit===this || this.contains(hit));}"#,
                vec![json!(x), json!(y)],
            )?;
            document(c, &t.document)?;
            c.call(
                "Input.dispatchMouseEvent",
                json!({"type":"mouseMoved","x":x,"y":y}),
            )?;
            c.call(
                "Input.dispatchMouseEvent",
                json!({"type":"mousePressed","x":x,"y":y,"button":"left","clickCount":1}),
            )?;
            c.call(
                "Input.dispatchMouseEvent",
                json!({"type":"mouseReleased","x":x,"y":y,"button":"left","clickCount":1}),
            )?;
        }
        _ => return Err("action required".into()),
    }
    let _ = c.call(
        "Runtime.releaseObjectGroup",
        json!({"objectGroup":"namzu-action"}),
    );
    Ok(Reply::Applied)
}
