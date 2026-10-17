use namzu_core::AgentError;
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    path::Path,
};

pub(super) fn read(path: &Path) -> Result<Vec<u8>, AgentError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|_| AgentError::State("cannot open regular upload file"))?;
    if !file
        .metadata()
        .map_err(|_| AgentError::State("cannot inspect upload file"))?
        .is_file()
    {
        return Err(AgentError::Configuration("regular upload file required"));
    }
    let mut bytes = vec![];
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| AgentError::State("cannot read upload file"))?;
    if bytes.len() > 65536 {
        return Err(AgentError::Configuration("upload exceeds 64 KiB"));
    }
    Ok(bytes)
}
pub(super) fn write_new(path: &Path, bytes: &[u8]) -> Result<(), AgentError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| AgentError::State("download target must be a new file"))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| AgentError::State("cannot save download"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binary_transfer_is_bounded_private_and_never_overwrites() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("binary");
        write_new(&path, &[0, 255, 10]).unwrap();
        assert_eq!(read(&path).unwrap(), [0, 255, 10]);
        assert!(write_new(&path, b"overwrite").is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::write(&path, vec![0; 65537]).unwrap();
        assert!(read(&path).is_err());
        assert!(read(root.path()).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn fifo_and_symlink_uploads_are_rejected_without_open_wait() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let fifo = root.path().join("pipe");
        let path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        assert!(read(&fifo).is_err());
        let real = root.path().join("real");
        std::fs::write(&real, b"secret").unwrap();
        let link = root.path().join("link");
        symlink(real, &link).unwrap();
        assert!(read(&link).is_err());
    }
}
