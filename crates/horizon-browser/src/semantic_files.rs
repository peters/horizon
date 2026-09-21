//! File attachment helpers shared by the Chromium and `WebDriver` engines:
//! the page probes that qualify an `input[type=file]`, the `accept` and
//! `multiple` checks, and the post-attachment readback that proves the input
//! holds the requested files.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::semantic::check_script_error;
use crate::{BrowserAttachedFile, BrowserControlFailure};

/// What the page reports about the attachment target before any file is set.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize)]
pub(crate) struct FileInputProbe {
    #[serde(default)]
    pub(crate) accept: String,
    #[serde(default)]
    pub(crate) multiple: bool,
}

/// JavaScript that qualifies the selector's element as an enabled
/// `input[type=file]` and reports its `accept` and `multiple` attributes.
/// Hidden inputs qualify: pages routinely hide the input behind a styled
/// button, so visibility is not required.
pub(crate) fn file_input_probe_expression(selector: &str) -> String {
    format!("({FILE_INPUT_PROBE_FUNCTION})({})", json_string(selector))
}

/// JavaScript that lists the files the selector's input currently holds.
pub(crate) fn attached_files_expression(selector: &str) -> String {
    format!("({ATTACHED_FILES_FUNCTION})({})", json_string(selector))
}

/// JavaScript whose value is the selector's element itself, for backends
/// that attach through an element handle rather than a script value.
pub(crate) fn element_handle_expression(selector: &str) -> String {
    format!("document.querySelector({})", json_string(selector))
}

pub(crate) fn parse_file_input_probe(value: &Value) -> Result<FileInputProbe, BrowserControlFailure> {
    check_script_error(value)?;
    serde_json::from_value(value.clone())
        .map_err(|error| BrowserControlFailure::new("invalid_result", format!("invalid file input probe: {error}")))
}

#[derive(serde::Deserialize)]
struct AttachedFiles {
    #[serde(default)]
    files: Vec<BrowserAttachedFile>,
}

pub(crate) fn parse_attached_files(value: &Value) -> Result<Vec<BrowserAttachedFile>, BrowserControlFailure> {
    check_script_error(value)?;
    serde_json::from_value::<AttachedFiles>(value.clone())
        .map(|attached| attached.files)
        .map_err(|error| BrowserControlFailure::new("invalid_result", format!("invalid attached file list: {error}")))
}

/// Refuse a request the input cannot take before any file is attached: more
/// than one file for a single-file input, or a file outside `accept`.
pub(crate) fn check_attachment_request(probe: &FileInputProbe, paths: &[PathBuf]) -> Result<(), BrowserControlFailure> {
    if paths.len() > 1 && !probe.multiple {
        return Err(BrowserControlFailure::new(
            "multiple_not_allowed",
            format!(
                "the file input accepts a single file but {} were requested",
                paths.len()
            ),
        ));
    }
    if let Some(rejected) = paths.iter().find(|path| !accept_allows(&probe.accept, path)) {
        return Err(BrowserControlFailure::new(
            "accept_mismatch",
            format!(
                "{} is outside the input's accept list ({})",
                rejected.display(),
                probe.accept.trim()
            ),
        ));
    }
    Ok(())
}

/// Every path must name an existing regular file on this host before the
/// backend is asked to read it.
pub(crate) fn check_local_files(paths: &[PathBuf]) -> Result<(), BrowserControlFailure> {
    for path in paths {
        match std::fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => {
                return Err(BrowserControlFailure::new(
                    "not_a_file",
                    format!("{} is not a regular file", path.display()),
                ));
            }
            Err(error) => {
                return Err(BrowserControlFailure::new(
                    "file_not_found",
                    format!("{} cannot be read: {error}", path.display()),
                ));
            }
        }
    }
    Ok(())
}

/// The input must hold exactly the requested files afterwards; a driver
/// that accepted the command without attaching anything is a failed action.
pub(crate) fn verify_attached(
    attached: &[BrowserAttachedFile],
    paths: &[PathBuf],
) -> Result<(), BrowserControlFailure> {
    let expected = paths
        .iter()
        .map(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()))
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default();
    let retained = attached.len() == expected.len()
        && expected
            .iter()
            .all(|name| attached.iter().any(|file| &file.name == name));
    if retained {
        Ok(())
    } else {
        Err(BrowserControlFailure::new(
            "attachment_mismatch",
            format!(
                "the file input holds {} file(s) after the attachment, expected {}",
                attached.len(),
                expected.len()
            ),
        ))
    }
}

/// Whether an HTML `accept` list admits `path`, judged by its extension:
/// `.ext` tokens match the extension directly, `type/subtype` and `type/*`
/// tokens match the MIME type derived from the extension. A file whose
/// extension is unknown is refused by a MIME-only list rather than guessed.
pub(crate) fn accept_allows(accept: &str, path: &Path) -> bool {
    let tokens = accept
        .split(',')
        .map(|token| token.trim().to_ascii_lowercase())
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return true;
    }
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase());
    let mime = extension.as_deref().and_then(mime_for_extension);
    tokens.iter().any(|token| {
        if let Some(wanted) = token.strip_prefix('.') {
            extension.as_deref() == Some(wanted)
        } else if let Some(family) = token.strip_suffix("/*") {
            mime.is_some_and(|mime| mime.split('/').next() == Some(family))
        } else {
            mime == Some(token.as_str())
        }
    })
}

fn mime_for_extension(extension: &str) -> Option<&'static str> {
    Some(match extension {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "tif" | "tiff" => "image/tiff",
        "txt" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "md" => "text/markdown",
        "json" => "application/json",
        "xml" => "application/xml",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "rtf" => "application/rtf",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "ogg" => "audio/ogg",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        _ => return None,
    })
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

const FILE_INPUT_PROBE_FUNCTION: &str = r"function(selector) {
    let element;
    try { element = document.querySelector(selector); }
    catch (error) { return { error: { code: 'invalid_selector', message: String(error?.message || error).slice(0, 512) } }; }
    if (!element) return { error: { code: 'no_such_element', message: 'no element matched the target' } };
    if (!(element instanceof HTMLInputElement) || element.type !== 'file')
        return { error: { code: 'not_file_input', message: 'target element is not an input[type=file]; target the file input itself, not the button that opens the chooser' } };
    if (element.matches(':disabled') || element.getAttribute('aria-disabled') === 'true')
        return { error: { code: 'element_disabled', message: 'target element is disabled' } };
    return { accept: String(element.getAttribute('accept') || '').slice(0, 2048), multiple: element.hasAttribute('multiple') };
}";

const ATTACHED_FILES_FUNCTION: &str = r"function(selector) {
    let element;
    try { element = document.querySelector(selector); }
    catch (error) { return { error: { code: 'invalid_selector', message: String(error?.message || error).slice(0, 512) } }; }
    if (!(element instanceof HTMLInputElement) || element.type !== 'file')
        return { error: { code: 'no_such_element', message: 'the file input is no longer in the document' } };
    return { files: Array.from(element.files || [], (file) => ({
        name: String(file.name).slice(0, 512), size: Number(file.size) || 0, mime: String(file.type).slice(0, 128)
    })) };
}";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn probe(accept: &str, multiple: bool) -> FileInputProbe {
        FileInputProbe {
            accept: accept.to_string(),
            multiple,
        }
    }

    #[test]
    fn accept_lists_match_by_extension_or_derived_mime() {
        let pdf = Path::new("/uploads/Claim Form.PDF");
        assert!(accept_allows("", pdf));
        assert!(accept_allows(" .pdf , image/* ", pdf));
        assert!(accept_allows("application/pdf", pdf));
        assert!(accept_allows("application/*", pdf));
        assert!(!accept_allows("image/*", pdf));
        assert!(!accept_allows(".png,.jpg", pdf));
        assert!(accept_allows("image/*", Path::new("/uploads/photo.HEIC")));
        assert!(!accept_allows("image/*", Path::new("/uploads/photo.unknownext")));
        assert!(accept_allows(".unknownext", Path::new("/uploads/photo.unknownext")));
        assert!(!accept_allows(".pdf", Path::new("/uploads/no-extension")));
    }

    #[test]
    fn attachment_requests_are_refused_before_any_file_is_set() {
        let one = vec![PathBuf::from("/uploads/a.pdf")];
        let two = vec![PathBuf::from("/uploads/a.pdf"), PathBuf::from("/uploads/b.png")];
        assert!(check_attachment_request(&probe("", false), &one).is_ok());
        assert!(check_attachment_request(&probe(".pdf,.png", true), &two).is_ok());
        assert_eq!(
            check_attachment_request(&probe("", false), &two).map_err(|error| error.code),
            Err("multiple_not_allowed".to_string())
        );
        let error = check_attachment_request(&probe(".pdf", true), &two).expect_err("png is outside accept");
        assert_eq!(error.code, "accept_mismatch");
        assert!(
            error.message.contains("b.png") && error.message.contains(".pdf"),
            "{}",
            error.message
        );
    }

    #[test]
    fn probes_and_readbacks_parse_or_surface_the_page_error() {
        let probe = parse_file_input_probe(&json!({ "accept": ".pdf", "multiple": true })).expect("probe");
        assert_eq!(
            probe,
            FileInputProbe {
                accept: ".pdf".into(),
                multiple: true
            }
        );
        let error = parse_file_input_probe(&json!({ "error": { "code": "not_file_input", "message": "no" } }))
            .expect_err("page error");
        assert_eq!(error.code, "not_file_input");
        let files =
            parse_attached_files(&json!({ "files": [{ "name": "a.pdf", "size": 12, "mime": "application/pdf" }] }))
                .expect("files");
        assert_eq!(files[0].name, "a.pdf");
        assert_eq!(files[0].size, 12);
        assert!(parse_attached_files(&json!({ "files": "nope" })).is_err());
    }

    #[test]
    fn readback_must_hold_exactly_the_requested_files() {
        let paths = vec![PathBuf::from("/uploads/a.pdf"), PathBuf::from("/uploads/b.png")];
        let attached = |names: &[&str]| {
            names
                .iter()
                .map(|name| BrowserAttachedFile {
                    name: (*name).to_string(),
                    size: 1,
                    mime: String::new(),
                })
                .collect::<Vec<_>>()
        };
        assert!(verify_attached(&attached(&["b.png", "a.pdf"]), &paths).is_ok());
        for retained in [&["a.pdf"][..], &["a.pdf", "c.txt"], &[]] {
            let error = verify_attached(&attached(retained), &paths).expect_err("mismatch");
            assert_eq!(error.code, "attachment_mismatch");
        }
    }

    #[test]
    fn local_files_must_exist_as_regular_files() {
        let directory = tempfile::tempdir().expect("tempdir");
        let file = directory.path().join("doc.pdf");
        std::fs::write(&file, b"%PDF-1.4").expect("write");
        assert!(check_local_files(std::slice::from_ref(&file)).is_ok());
        assert_eq!(
            check_local_files(&[directory.path().to_path_buf()]).map_err(|error| error.code),
            Err("not_a_file".to_string())
        );
        assert_eq!(
            check_local_files(&[file, directory.path().join("missing.pdf")]).map_err(|error| error.code),
            Err("file_not_found".to_string())
        );
    }

    #[test]
    fn page_expressions_quote_the_selector() {
        assert!(file_input_probe_expression("input[name=\"doc\"]").contains("(\"input[name=\\\"doc\\\"]\")"));
        assert!(attached_files_expression("#doc").ends_with("(\"#doc\")"));
        assert_eq!(element_handle_expression("#doc"), "document.querySelector(\"#doc\")");
    }
}
