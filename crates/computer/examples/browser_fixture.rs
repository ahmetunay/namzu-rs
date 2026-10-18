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
    let (status, extra, body) = match path {
        "/" => (
            "200 OK",
            "",
            String::from(
                "<!doctype html><html lang='en'><head><meta charset='utf-8'><title>Namzu Browser Fixture</title><style>body{margin:0;background:rgb(51,102,153)}</style></head><body><h1>Kontrol paneli</h1><label for='name'>Your name</label><input id='name' value='initial'><button aria-label='Run task'>Run</button><a href='/next'>Next page</a><p>Deterministic content ü</p></body></html>",
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
