//! Deterministic HTTP oracle, installed only in the acceptance fixture image.
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
};
fn main() -> std::io::Result<()> {
    let server = TcpListener::bind("127.0.0.1:8765")?;
    std::fs::write("/workspace/browser-ready", b"ready")?;
    for stream in server.incoming().flatten() {
        // Chromium may hold idle preconnects. They must not block or kill the fixture.
        std::thread::spawn(move || {
            let _ = serve(stream);
        });
    }
    Ok(())
}
fn serve(mut stream: TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        if bytes.len() >= 8192 {
            return Ok(());
        }
        let mut byte = [0u8; 1];
        stream.read_exact(&mut byte)?;
        bytes.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&bytes);
    let path = text.split_whitespace().nth(1).unwrap_or("/");
    if path == "/upload" {
        let length = text
            .lines()
            .find_map(|l| {
                l.split_once(':')
                    .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, v)| v.trim().parse::<usize>().ok())
            })
            .unwrap_or(0);
        if length > 65536 {
            return Ok(());
        }
        let mut body = vec![0u8; length];
        stream.read_exact(&mut body)?;
        std::fs::write("/workspace/upload-received.bin", &body)?;
        let response = format!("uploaded:{length}");
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
            response.len()
        )?;
        return Ok(());
    }
    if matches!(path, "/download" | "/large-download" | "/slow-download") {
        let body = if path == "/large-download" {
            vec![7u8; 65537]
        } else {
            vec![0, 255, 10, 13, 42]
        };
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"../../untrusted.bin\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )?;
        if path == "/slow-download" {
            stream.write_all(&body[..1])?;
            stream.flush()?;
            std::fs::write("/workspace/download-started", b"started")?;
            std::thread::sleep(std::time::Duration::from_secs(10));
            stream.write_all(&body[1..])?;
        } else {
            stream.write_all(&body)?;
        }
        return Ok(());
    }
    let (status, extra, body) = match path {
        "/" => (
            "200 OK",
            "",
            String::from(
                "<!doctype html><html lang='en'><head><meta charset='utf-8'><title>Namzu Browser Fixture</title><style>body{margin:0;background:rgb(51,102,153)}</style></head><body><h1>Kontrol paneli</h1><label for='name'>Your name</label><input id='name' value='initial'><button aria-label='Run task'>Run</button><a href='/next'>Next page</a><p>Deterministic content ü</p></body></html>",
            ),
        ),
        "/actions" => (
            "200 OK",
            "",
            String::from(
                r#"<!doctype html><html><head><meta charset='utf-8'><title>Action Fixture</title></head><body>
<form onsubmit="event.preventDefault();document.getElementById('status').textContent='submitted'">
<label for='edit'>Editable</label><input id='edit' value='initial'><textarea id='notes'></textarea><fieldset disabled><input id='blocked' value='untouched'></fieldset>
<select id='choice' onchange="document.getElementById('status').textContent='choice:'+this.value"><option value='a'>Alpha</option><option value='b'>Beta</option></select>
<button id='run' type='button' onclick="document.getElementById('status').textContent='clicked:'+document.getElementById('edit').value">Run</button>
<input id='file' type='file'><button id='send' type='button' onclick="fetch('/upload',{method:'POST',body:document.getElementById('file').files[0]}).then(r=>r.text()).then(t=>document.getElementById('status').textContent=t)">Send file</button><button type="submit">Submit</button>
</form><p id='status'>ready</p><a id='next' href='/next'>Next page</a>
<div style='height:2000px'></div><button id='bottom' onclick="document.getElementById('status').textContent='bottom'">Bottom</button>
</body></html>"#,
            ),
        ),
        "/next" => (
            "200 OK",
            "",
            String::from(
                "<!doctype html><html><head><title>Second Page</title><style>body{margin:0;background:rgb(68,170,102)}</style></head><body><h1>Second page</h1><p>Navigation complete</p></body></html>",
            ),
        ),
        "/redirect" => ("302 Found", "Location: /next\r\n", String::new()),
        "/large" => (
            "200 OK",
            "",
            format!(
                "<!doctype html><html><head><title>Large Page</title></head><body>{}</body></html>",
                (0..1000)
                    .map(|i| format!("<p>Item {i}</p>"))
                    .collect::<String>()
            ),
        ),
        _ => ("404 Not Found", "", String::from("Not found")),
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{body}",
        body.len()
    )?;
    Ok(())
}
