//! Decode bounded native file transfer payloads into local paths.
use std::io::Write;
use std::path::PathBuf;
const URI_LIST: &str = "text/uri-list";
/// Decode local file URLs or persist an encoded PNG/JPEG in a private file.
///
/// # Errors
/// Returns an error for unsupported formats, unsafe URLs, or failed file writes.
pub fn decode_transfer_payload(mime: &str, bytes: &[u8]) -> std::io::Result<Vec<PathBuf>> {
    if bytes.len() > 32 * 1024 * 1024 {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    if mime == URI_LIST {
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
        return Ok(paths);
    }
    let suffix = match mime {
        "image/png" if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => ".png",
        "image/jpeg" if bytes.starts_with(&[0xff, 0xd8, 0xff]) => ".jpg",
        _ => return Err(std::io::ErrorKind::InvalidData.into()),
    };
    let mut file = tempfile::Builder::new()
        .prefix("horizon-image-")
        .suffix(suffix)
        .tempfile()?;
    file.write_all(bytes)?;
    file.flush()?;
    let (_, path) = file.keep().map_err(|error| error.error)?;
    Ok(vec![path])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_lists_decode_local_paths_without_accepting_remote_urls() {
        assert_eq!(
            decode_transfer_payload(
                URI_LIST,
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
        ] {
            assert!(decode_transfer_payload(URI_LIST, invalid.as_bytes()).is_err());
        }
    }

    #[test]
    fn image_payloads_are_private_files_and_wrong_formats_are_rejected() {
        use std::os::unix::fs::PermissionsExt;
        let paths = decode_transfer_payload("image/png", b"\x89PNG\r\n\x1a\nfixture").unwrap();
        assert_eq!(
            std::fs::metadata(&paths[0]).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(std::fs::read(&paths[0]).unwrap(), b"\x89PNG\r\n\x1a\nfixture");
        std::fs::remove_file(&paths[0]).unwrap();
        assert!(decode_transfer_payload("image/png", b"not an image").is_err());
    }
}
