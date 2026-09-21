//! Transfer authorized staged files to a remote session before selecting them.
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use crate::BrowserControlFailure;
use crate::webdriver::http::HttpError;
use crate::webdriver::transport::ClassicTransport;

/// Bound the in-memory JSON/base64 envelope as well as provider storage.
use crate::{MAX_REMOTE_ATTACHMENT_BYTES as MAX_FILE_BYTES, MAX_REMOTE_ATTACHMENT_REQUEST_BYTES as MAX_REQUEST_BYTES};
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(40);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Transfer {
    Selenium,
    Android,
}

impl Transfer {
    pub(super) fn for_provider(adapter: crate::remote::RemoteAdapterKind, platform: Option<&str>) -> Option<Self> {
        if adapter != crate::remote::RemoteAdapterKind::Browserstack {
            return None;
        }
        Self::for_platform(platform)
    }

    fn for_platform(platform: Option<&str>) -> Option<Self> {
        match platform?.to_ascii_lowercase().as_str() {
            "android" => Some(Self::Android),
            "mac" | "macos" | "mac os x" | "os x" | "windows" | "win32" | "win64" | "linux" => Some(Self::Selenium),
            _ => None,
        }
    }

    pub(super) fn upload(
        self,
        transport: &dyn ClassicTransport,
        session_id: &str,
        paths: &[PathBuf],
    ) -> Result<Vec<PathBuf>, BrowserControlFailure> {
        // Validate the whole batch before sending any file bytes.
        let mut total = 0u64;
        for path in paths {
            let size = path.metadata().map_err(|_| failed())?.len();
            total = total.saturating_add(size);
            if size > MAX_FILE_BYTES || total > MAX_REQUEST_BYTES {
                return Err(too_large());
            }
        }
        let deadline = Instant::now() + TRANSFER_TIMEOUT;
        let batch = uuid::Uuid::new_v4();
        paths
            .iter()
            .enumerate()
            .map(|(index, path)| {
                let name = path.file_name().and_then(|name| name.to_str()).ok_or_else(failed)?;
                let bytes = read_bounded(path)?;
                let (suffix, payload) = match self {
                    Self::Selenium => ("se/file", json!({"file": STANDARD.encode(zip_file(name, &bytes)?)})),
                    Self::Android => (
                        "appium/device/push_file",
                        json!({
                            "path": format!("/data/local/tmp/horizon-{batch}/{index}/{name}"),
                            "data": STANDARD.encode(&bytes),
                        }),
                    ),
                };
                let response = post(transport, session_id, suffix, &payload, deadline);
                // Legacy Selenium grids expose /file. Retry only an explicit
                // unsupported-command refusal, never an ambiguous transport error.
                let response = match response {
                    Err(HttpError::WebDriver { ref error, .. })
                        if self == Self::Selenium
                            && matches!(error.as_str(), "unknown command" | "unsupported operation") =>
                    {
                        post(transport, session_id, "file", &payload, deadline)
                    }
                    other => other,
                }
                .map_err(|error| {
                    if matches!(&error, HttpError::WebDriver { error, .. } if matches!(error.as_str(), "unknown command" | "unsupported operation")) {
                        BrowserControlFailure::new("unsupported_backend", "the remote provider does not implement file transfer")
                    } else {
                        failed()
                    }
                })?;
                match self {
                    Self::Android => Ok(PathBuf::from(payload["path"].as_str().ok_or_else(failed)?)),
                    Self::Selenium => {
                        let remote = response.get("value").and_then(Value::as_str).ok_or_else(failed)?;
                        if remote.is_empty() || remote.len() > 4096 || remote.chars().any(char::is_control) {
                            return Err(failed());
                        }
                        Ok(PathBuf::from(remote))
                    }
                }
            })
            .collect()
    }
}

fn post(
    transport: &dyn ClassicTransport,
    session: &str,
    suffix: &str,
    payload: &Value,
    deadline: Instant,
) -> Result<Value, HttpError> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| HttpError::Transport("file transfer deadline elapsed".into()))?;
    transport.post_with_read_timeout(&format!("/session/{session}/{suffix}"), payload, remaining)
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, BrowserControlFailure> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| failed())?
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| failed())?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(too_large());
    }
    Ok(bytes)
}

fn zip_file(name: &str, bytes: &[u8]) -> Result<Vec<u8>, BrowserControlFailure> {
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    archive
        .start_file(
            name,
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .map_err(|_| failed())?;
    archive.write_all(bytes).map_err(|_| failed())?;
    archive.finish().map(Cursor::into_inner).map_err(|_| failed())
}

fn failed() -> BrowserControlFailure {
    // Provider responses can echo the request, including file bytes. Never
    // include them in errors or audit records.
    BrowserControlFailure::new(
        "attachment_transfer_failed",
        "remote file transfer failed; no file selection was requested",
    )
}

fn too_large() -> BrowserControlFailure {
    BrowserControlFailure::new(
        "file_too_large",
        "remote attachments allow at most 16 MiB per file and 32 MiB per request",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Transport {
        calls: Mutex<Vec<(String, Value)>>,
    }
    impl ClassicTransport for Transport {
        fn request(&self, method: &str, path: &str, body: Option<&Value>, _: Duration) -> Result<Value, HttpError> {
            assert_eq!(method, "POST");
            self.calls
                .lock()
                .expect("calls")
                .push((path.into(), body.expect("body").clone()));
            Ok(json!({"value": "/remote/upload/notes.txt"}))
        }
    }

    #[test]
    fn desktop_upload_preserves_name_and_bytes_in_a_zip_envelope() {
        let root = tempfile::tempdir().expect("root");
        let path = root.path().join("notes.txt");
        std::fs::write(&path, "unique content").expect("file");
        let transport = Transport::default();
        let remote = Transfer::Selenium
            .upload(&transport, "session", &[path])
            .expect("upload");
        assert_eq!(remote, [PathBuf::from("/remote/upload/notes.txt")]);
        let calls = transport.calls.lock().expect("calls");
        assert_eq!(calls[0].0, "/session/session/se/file");
        let zip = STANDARD
            .decode(calls[0].1["file"].as_str().expect("encoded"))
            .expect("base64");
        let mut archive = zip::ZipArchive::new(Cursor::new(zip)).expect("zip");
        let mut file = archive.by_name("notes.txt").expect("basename");
        let mut content = String::new();
        file.read_to_string(&mut content).expect("contents");
        assert_eq!(content, "unique content");
    }

    #[test]
    fn android_pushes_content_to_unique_paths_with_the_original_basename() {
        let root = tempfile::tempdir().expect("root");
        let path = root.path().join("notes.txt");
        std::fs::write(&path, "bytes").expect("file");
        let transport = Transport::default();
        let paths = Transfer::Android
            .upload(&transport, "session", &[path.clone(), path])
            .expect("upload");
        assert_ne!(paths[0], paths[1]);
        assert!(
            paths
                .iter()
                .all(|path| path.file_name() == Some(std::ffi::OsStr::new("notes.txt")))
        );
        let calls = transport.calls.lock().expect("calls");
        assert_eq!(calls[0].0, "/session/session/appium/device/push_file");
        assert_eq!(
            STANDARD
                .decode(calls[0].1["data"].as_str().expect("encoded"))
                .expect("base64"),
            b"bytes"
        );
    }

    #[test]
    fn unsupported_platforms_and_oversized_files_never_transfer() {
        assert_eq!(
            Transfer::for_provider(crate::remote::RemoteAdapterKind::Webdriver, Some("Windows")),
            None
        );
        assert_eq!(
            Transfer::for_provider(crate::remote::RemoteAdapterKind::Webdriver, Some("Android")),
            None
        );
        assert_eq!(
            Transfer::for_provider(crate::remote::RemoteAdapterKind::Browserstack, Some("OS X")),
            Some(Transfer::Selenium)
        );
        assert_eq!(Transfer::for_platform(Some("iOS")), None);
        assert_eq!(Transfer::for_platform(None), None);
        assert_eq!(Transfer::for_platform(Some("OS X")), Some(Transfer::Selenium));
        assert_eq!(Transfer::for_platform(Some("Android")), Some(Transfer::Android));
        let root = tempfile::tempdir().expect("root");
        let path = root.path().join("large.txt");
        std::fs::File::create(&path)
            .expect("file")
            .set_len(MAX_FILE_BYTES + 1)
            .expect("length");
        let transport = Transport::default();
        assert_eq!(
            Transfer::Selenium
                .upload(&transport, "session", &[path])
                .expect_err("too large")
                .code,
            "file_too_large"
        );
        assert!(transport.calls.lock().expect("calls").is_empty());
    }
    struct RefusingTransport {
        calls: Mutex<Vec<String>>,
        ambiguous: bool,
        legacy_supported: bool,
    }

    impl ClassicTransport for RefusingTransport {
        fn request(&self, _: &str, path: &str, _: Option<&Value>, _: Duration) -> Result<Value, HttpError> {
            self.calls.lock().expect("calls").push(path.into());
            if self.ambiguous {
                Err(HttpError::Transport("secret echoed bytes".into()))
            } else if path.ends_with("/se/file") || !self.legacy_supported {
                Err(HttpError::WebDriver {
                    error: "unknown command".into(),
                    message: "private response".into(),
                })
            } else {
                Ok(json!({"value": "/remote/notes.txt"}))
            }
        }
    }

    #[test]
    fn legacy_fallback_requires_a_definite_refusal_and_errors_do_not_echo_content() {
        let root = tempfile::tempdir().expect("root");
        let path = root.path().join("notes.txt");
        std::fs::write(&path, "bytes").expect("file");
        for (ambiguous, legacy_supported) in [(false, true), (true, true), (false, false)] {
            let transport = RefusingTransport {
                calls: Mutex::default(),
                ambiguous,
                legacy_supported,
            };
            let result = Transfer::Selenium.upload(&transport, "session", std::slice::from_ref(&path));
            let calls = transport.calls.lock().expect("calls");
            if ambiguous {
                let error = result.expect_err("uncertain transfer must not retry");
                assert_eq!(error.code, "attachment_transfer_failed");
                assert!(!error.message.contains("secret"));
                assert_eq!(calls.len(), 1);
            } else if !legacy_supported {
                assert_eq!(result.expect_err("unsupported endpoints").code, "unsupported_backend");
                assert_eq!(calls.len(), 2);
            } else {
                assert_eq!(result.expect("legacy upload"), [PathBuf::from("/remote/notes.txt")]);
                assert_eq!(*calls, ["/session/session/se/file", "/session/session/file"]);
            }
        }
    }
}
