//! Decode bounded native file transfer payloads into local paths.
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
/// Owns private images until the native application exits.
#[derive(Default)]
pub struct TransferFiles {
    directory: std::sync::OnceLock<tempfile::TempDir>,
}
impl TransferFiles {
    /// Remove owned images before a host exit path that skips destructors.
    ///
    /// # Errors
    /// Returns an error if the private directory cannot be removed.
    pub fn clear(&mut self) -> std::io::Result<()> {
        self.directory.take().map_or(Ok(()), tempfile::TempDir::close)
    }

    const URI_LIST: &str = "text/uri-list";
    /// Decode local file URLs or persist an encoded PNG/JPEG in a private file.
    ///
    /// # Errors
    /// Returns an error for unsupported formats, unsafe URLs, or failed file writes.
    pub fn decode_transfer_payload(&self, mime: &str, bytes: &[u8]) -> std::io::Result<Vec<PathBuf>> {
        if bytes.len() > 32 * 1024 * 1024 {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        if mime == Self::URI_LIST {
            let text = std::str::from_utf8(bytes).map_err(|_| std::io::ErrorKind::InvalidData)?;
            let mut paths = Vec::new();
            for line in text.lines().filter(|line| !line.is_empty() && !line.starts_with('#')) {
                let uri = url::Url::parse(line).map_err(|_| std::io::ErrorKind::InvalidData)?;
                let path = uri.to_file_path().map_err(|()| std::io::ErrorKind::InvalidData)?;
                if !path.is_absolute() || path.as_os_str().as_encoded_bytes().contains(&0) || paths.len() >= 1024 {
                    return Err(std::io::ErrorKind::InvalidData.into());
                }
                paths.push(path);
            }
            if paths.is_empty() {
                return Err(std::io::ErrorKind::InvalidData.into());
            }
            return Ok(paths);
        }
        let suffix = match mime {
            "image/png" if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => ".png",
            "image/jpeg" if bytes.starts_with(&[0xff, 0xd8, 0xff]) => ".jpg",
            _ => return Err(std::io::ErrorKind::InvalidData.into()),
        };
        if self.directory.get().is_none() {
            let directory = tempfile::Builder::new()
                .prefix("horizon-images-")
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir()?;
            let _ = self.directory.set(directory);
        }
        let directory = self
            .directory
            .get()
            .ok_or_else(|| std::io::Error::other("image directory unavailable"))?;
        let mut file = tempfile::Builder::new()
            .prefix("horizon-image-")
            .suffix(suffix)
            .tempfile_in(directory.path())?;
        file.write_all(bytes)?;
        file.flush()?;
        let (_, path) = file.keep().map_err(|error| error.error)?;
        Ok(vec![path])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_lists_decode_local_paths_without_accepting_remote_urls() {
        assert_eq!(
            TransferFiles::default()
                .decode_transfer_payload(
                    TransferFiles::URI_LIST,
                    b"# files\r\nfile:///tmp/a%20b.png\r\nfile://localhost/tmp/c.jpg\r\n"
                )
                .unwrap(),
            vec![PathBuf::from("/tmp/a b.png"), PathBuf::from("/tmp/c.jpg")]
        );
        for invalid in [
            "https://example.com/image.png",
            "file://remote/tmp/image.png",
            "file:///tmp/a%00b",
            "broken",
            "",
            "# comment only\r\n",
        ] {
            assert!(
                TransferFiles::default()
                    .decode_transfer_payload(TransferFiles::URI_LIST, invalid.as_bytes())
                    .is_err()
            );
        }
    }

    #[test]
    fn image_payloads_are_private_files_and_wrong_formats_are_rejected() {
        use std::os::unix::fs::PermissionsExt;
        let files = TransferFiles::default();
        let paths = files
            .decode_transfer_payload("image/png", b"\x89PNG\r\n\x1a\nfixture")
            .unwrap();
        assert_eq!(
            std::fs::metadata(&paths[0]).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(std::fs::read(&paths[0]).unwrap(), b"\x89PNG\r\n\x1a\nfixture");
        let directory = paths[0].parent().unwrap().to_path_buf();
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let mut files = files;
        files.clear().unwrap();
        files.clear().unwrap();
        assert!(!paths[0].exists());
        assert!(!directory.exists());
        assert!(
            TransferFiles::default()
                .decode_transfer_payload("image/png", b"not an image")
                .is_err()
        );
    }
}
