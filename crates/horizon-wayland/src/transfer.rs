//! Decode bounded native file transfer payloads into local paths.
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
/// Owns private images until the native application exits.
#[derive(Default)]
pub struct TransferFiles {
    directory: std::sync::Mutex<Directory>,
}
#[derive(Default)]
struct Directory {
    owned: Option<tempfile::TempDir>,
    closed: bool,
    generation: u64,
}
impl TransferFiles {
    /// Remove owned images before a host exit path that skips destructors.
    ///
    /// # Errors
    /// Returns an error if the private directory cannot be removed.
    pub fn clear(&self) -> std::io::Result<()> {
        let mut directory = self.directory.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        directory.closed = true;
        directory.owned.take().map_or(Ok(()), tempfile::TempDir::close)
    }

    /// Invalidate pending image writes while keeping previously delivered files alive.
    pub fn set_generation(&self, generation: u64) {
        self.directory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .generation = generation;
    }

    const URI_LIST: &str = "text/uri-list";
    /// Decode local file URLs or persist an encoded PNG/JPEG in a private file.
    ///
    /// # Errors
    /// Returns an error for unsupported formats, unsafe URLs, or failed file writes.
    pub fn decode_transfer_payload(&self, mime: &str, bytes: &[u8]) -> std::io::Result<Vec<PathBuf>> {
        self.decode_transfer_payload_for_generation(0, mime, bytes)
    }

    /// Decode a payload only for the current session generation.
    ///
    /// # Errors
    /// Returns an error for stale sessions, invalid formats, or failed file writes.
    pub fn decode_transfer_payload_for_generation(
        &self,
        generation: u64,
        mime: &str,
        bytes: &[u8],
    ) -> std::io::Result<Vec<PathBuf>> {
        {
            let owner = self.directory.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if owner.closed || owner.generation != generation {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
        }
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
        let (suffix, format) = match mime {
            "image/png" => (".png", image::ImageFormat::Png),
            "image/jpeg" => (".jpg", image::ImageFormat::Jpeg),
            _ => return Err(std::io::ErrorKind::InvalidData.into()),
        };
        let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(16384);
        limits.max_image_height = Some(16384);
        limits.max_alloc = Some(256 * 1024 * 1024);
        reader.limits(limits);
        reader
            .decode()
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let mut owner = self.directory.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if owner.closed || owner.generation != generation {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        if owner.owned.is_none() {
            owner.owned = Some(
                tempfile::Builder::new()
                    .prefix("horizon-images-")
                    .permissions(std::fs::Permissions::from_mode(0o700))
                    .tempdir()?,
            );
        }
        let directory = owner
            .owned
            .as_ref()
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
    fn session_reset_rejects_stale_persistence_without_removing_delivered_files() {
        let files = TransferFiles::default();
        let png = include_bytes!("fixtures/image.png");
        let delivered = files.decode_transfer_payload("image/png", png).unwrap();
        files.set_generation(1);
        assert!(
            files
                .decode_transfer_payload_for_generation(0, "image/png", png)
                .is_err()
        );
        assert!(delivered[0].exists());
        assert_eq!(std::fs::read_dir(delivered[0].parent().unwrap()).unwrap().count(), 1);
        assert!(
            files
                .decode_transfer_payload_for_generation(1, "image/png", png)
                .unwrap()[0]
                .exists()
        );
        files.clear().unwrap();
    }

    #[test]
    fn jpeg_payloads_validate_and_oversized_dimensions_are_rejected() {
        let files = TransferFiles::default();
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode(&[0, 255, 255], 1, 1, image::ExtendedColorType::Rgb8)
            .unwrap();
        assert!(files.decode_transfer_payload("image/jpeg", &jpeg).unwrap()[0].exists());
        let mut png = Vec::new();
        image::ImageEncoder::write_image(
            image::codecs::png::PngEncoder::new(&mut png),
            &vec![0; 16385 * 3],
            16385,
            1,
            image::ExtendedColorType::Rgb8,
        )
        .unwrap();
        assert!(files.decode_transfer_payload("image/png", &png).is_err());
    }

    #[test]
    fn truncated_images_with_valid_signatures_are_rejected() {
        let files = TransferFiles::default();
        for (mime, bytes) in [
            ("image/png", &b"\x89PNG\r\n\x1a\nfixture"[..]),
            ("image/jpeg", &b"\xff\xd8\xfffixture"[..]),
        ] {
            assert!(files.decode_transfer_payload(mime, bytes).is_err());
        }
        assert!(files.directory.lock().unwrap().owned.is_none());
    }

    #[test]
    fn image_payloads_are_private_files_and_wrong_formats_are_rejected() {
        use std::os::unix::fs::PermissionsExt;
        let files = TransferFiles::default();
        let paths = files
            .decode_transfer_payload("image/png", include_bytes!("fixtures/image.png"))
            .unwrap();
        assert_eq!(
            std::fs::metadata(&paths[0]).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(std::fs::read(&paths[0]).unwrap(), include_bytes!("fixtures/image.png"));
        let directory = paths[0].parent().unwrap().to_path_buf();
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        files.clear().unwrap();
        files.clear().unwrap();
        assert!(!paths[0].exists());
        assert!(!directory.exists());
        assert!(
            files
                .decode_transfer_payload("image/png", include_bytes!("fixtures/image.png"))
                .is_err()
        );
        assert!(
            TransferFiles::default()
                .decode_transfer_payload("image/png", b"not an image")
                .is_err()
        );
    }
}
