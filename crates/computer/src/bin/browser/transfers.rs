//! Bounded guest-only transfers. Suggested remote filenames are never paths.
use super::{Cdp, Result, json};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions, OpenOptionsExt},
};
use namzu_computer::{
    browser::{Document, Reply},
    protocol::FILE_LIMIT,
};
use std::os::unix::fs::OpenOptionsExt as _;
use std::{
    io::{Read, Write},
    os::unix::fs::MetadataExt,
};
fn read_bounded(root: &Dir, path: &str) -> Result<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true).custom_flags(libc::O_NONBLOCK);
    let f = root.open_with(path, &options)?;
    let m = f.metadata()?;
    if !m.is_file() || m.len() > FILE_LIMIT as u64 {
        return Err("bounded regular upload file required".into());
    }
    let mut bytes = Vec::new();
    f.take((FILE_LIMIT + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > FILE_LIMIT {
        return Err("upload byte limit".into());
    }
    Ok(bytes)
}
pub fn stage_upload(path: &str) -> Result<tempfile::TempDir> {
    let count = std::fs::read_dir("/tmp")?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("namzu-upload-"))
        .count();
    if count >= 16 {
        return Err("upload staging session limit".into());
    }
    let root = Dir::open_ambient_dir("/workspace", ambient_authority())?;
    let bytes = read_bounded(&root, path)?;
    let staged = tempfile::Builder::new()
        .prefix("namzu-upload-")
        .tempdir_in("/tmp")?;
    let file = staged
        .path()
        .join(std::path::Path::new(path).file_name().ok_or("file name")?);
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(file)?;
    out.write_all(&bytes)?;
    out.sync_all()?;
    Ok(staged)
}
fn publish(root: &Dir, path: &str, bytes: &[u8]) -> Result<()> {
    let path = std::path::Path::new(path);
    let leaf = path.file_name().ok_or("file name")?;
    let parent = root.open_dir(
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(std::path::Path::new(".")),
    )?;
    let temp = format!(".namzu-download-{}", std::process::id());
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = parent.open_with(&temp, &options)?;
    let result = (|| -> Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        parent.hard_link(&temp, &parent, leaf)?;
        parent.open(".")?.sync_all()?;
        Ok(())
    })();
    let cleanup = parent.remove_file(&temp);
    result?;
    cleanup?;
    Ok(())
}
pub fn download(
    c: &mut Cdp,
    control: &mut Cdp,
    document: &Document,
    url: &str,
    path: &str,
) -> Result<Reply> {
    super::actions::document(c, document)?;
    // Reject existing outputs before any browser effect; publication also uses no-overwrite.
    let root = Dir::open_ambient_dir("/workspace", ambient_authority())?;
    if root.symlink_metadata(path).is_ok() {
        return Err("download output already exists".into());
    }
    let output = std::path::Path::new(path);
    root.open_dir(
        output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(std::path::Path::new(".")),
    )?;
    let directory = tempfile::Builder::new()
        .prefix("namzu-download-")
        .tempdir_in("/tmp")?;
    control.clear_events();
    control.call(
        "Browser.setDownloadBehavior",
        json!({"behavior":"allowAndName","downloadPath":directory.path(),"eventsEnabled":true}),
    )?;
    let result = (|| -> Result<Vec<u8>> {
        let result = c.call("Page.navigate", json!({"url":url}))?;
        if result["isDownload"] != true {
            return Err("URL did not initiate a download".into());
        }
        let mut guid = None;
        loop {
            let event = control.next_event()?;
            match event["method"].as_str() {
                Some("Browser.downloadWillBegin") => {
                    let id = event["params"]["guid"].as_str().ok_or("download id")?;
                    if guid.is_some()
                        || event["params"]["frameId"] != document.frame_id
                        || id.len() > 64
                        || id.is_empty()
                        || !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
                    {
                        return Err("unexpected download".into());
                    }
                    guid = Some(id.to_owned());
                }
                Some("Browser.downloadProgress") => {
                    let id = guid.as_deref().ok_or("download progress before start")?;
                    if event["params"]["guid"] != id {
                        return Err("download id mismatch".into());
                    }
                    if ["receivedBytes", "totalBytes"].iter().any(|k| {
                        event["params"][k]
                            .as_f64()
                            .is_none_or(|n| !n.is_finite() || n < 0.0 || n > FILE_LIMIT as f64)
                    }) {
                        let _ = control.call("Browser.cancelDownload", json!({"guid":id}));
                        return Err("download byte limit".into());
                    }
                    match event["params"]["state"].as_str() {
                        Some("completed") => {
                            let mut options = std::fs::OpenOptions::new();
                            options
                                .read(true)
                                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
                            let f = options.open(directory.path().join(id))?;
                            let meta = f.metadata()?;
                            if !meta.is_file()
                                || meta.nlink() != 1
                                || meta.len() > FILE_LIMIT as u64
                            {
                                return Err("invalid downloaded file".into());
                            }
                            let mut bytes = Vec::new();
                            f.take((FILE_LIMIT + 1) as u64).read_to_end(&mut bytes)?;
                            if bytes.len() > FILE_LIMIT {
                                return Err("download byte limit".into());
                            }
                            return Ok(bytes);
                        }
                        Some("canceled") => return Err("download cancelled".into()),
                        Some("inProgress") => {}
                        _ => return Err("unknown download state".into()),
                    }
                }
                _ => {}
            }
        }
    })();
    // Restore deny before publishing. Any failure retains the host's effect fence.
    control.call(
        "Browser.setDownloadBehavior",
        json!({"behavior":"deny","eventsEnabled":true}),
    )?;
    let bytes = result?;
    publish(&root, path, &bytes)?;
    Ok(Reply::Downloaded {
        path: path.into(),
        bytes: bytes.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uploads_are_regular_bounded_and_cannot_escape_the_root() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("binary"), [0, 255, 10]).unwrap();
        std::fs::write(temp.path().join("large"), vec![0u8; FILE_LIMIT + 1]).unwrap();
        std::fs::write(outside.path().join("private"), b"outside").unwrap();
        std::os::unix::fs::symlink(outside.path().join("private"), temp.path().join("escape"))
            .unwrap();
        let root = Dir::open_ambient_dir(temp.path(), ambient_authority()).unwrap();
        assert_eq!(read_bounded(&root, "binary").unwrap(), [0, 255, 10]);
        assert!(read_bounded(&root, "large").is_err());
        assert!(read_bounded(&root, "escape").is_err());
        assert!(read_bounded(&root, ".").is_err());
    }
    #[test]
    fn downloaded_files_publish_atomically_and_never_overwrite_or_escape() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = Dir::open_ambient_dir(temp.path(), ambient_authority()).unwrap();
        publish(&root, "result", b"first").unwrap();
        assert!(publish(&root, "result", b"second").is_err());
        assert_eq!(std::fs::read(temp.path().join("result")).unwrap(), b"first");
        assert_eq!(
            std::fs::metadata(temp.path().join("result"))
                .unwrap()
                .mode()
                & 0o777,
            0o600
        );
        std::os::unix::fs::symlink(outside.path(), temp.path().join("escape")).unwrap();
        assert!(publish(&root, "escape/result", b"private").is_err());
        assert!(!outside.path().join("result").exists());
        assert!(!std::fs::read_dir(temp.path()).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".namzu-download-")
        }));
    }
}
