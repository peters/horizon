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
    /// Length of the whole `accept` attribute, which may exceed what the
    /// probe carried; such a list is refused rather than judged in part.
    #[serde(default, rename = "acceptLength")]
    pub(crate) accept_length: usize,
    #[serde(default)]
    pub(crate) multiple: bool,
}

/// Longest `accept` attribute the probe carries and judges in full.
pub(crate) const MAX_ACCEPT_CHARACTERS: usize = 8192;

/// JavaScript that qualifies the selector's element as an enabled
/// `input[type=file]` and reports its `accept` and `multiple` attributes.
/// Hidden inputs qualify: pages routinely hide the input behind a styled
/// button, so visibility is not required.
pub(crate) fn file_input_probe_expression(selector: &str) -> String {
    format!("({FILE_INPUT_PROBE_FUNCTION})({})", json_string(selector))
}

/// JavaScript that drops the selector's current file selection, so a
/// backend whose Send Keys appends to a `multiple` input replaces the
/// selection like the Chromium primitive does. Programmatic clearing fires
/// no events; the attachment that follows does.
pub(crate) fn reset_file_input_expression(selector: &str) -> String {
    format!("({RESET_FILE_INPUT_FUNCTION})({})", json_string(selector))
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
    if probe.accept_length > MAX_ACCEPT_CHARACTERS {
        return Err(BrowserControlFailure::new(
            "accept_unsupported",
            format!(
                "the input's accept list is {} characters long, above the {MAX_ACCEPT_CHARACTERS} this action judges in full",
                probe.accept_length
            ),
        ));
    }
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
        // Only the file name is reported: at this point the paths are the
        // private staged copies, which stay internal.
        return Err(BrowserControlFailure::new(
            "accept_mismatch",
            format!(
                "{} is outside the input's accept list ({})",
                rejected
                    .file_name()
                    .map(|name| name.to_string_lossy())
                    .unwrap_or_default(),
                probe.accept.trim()
            ),
        ));
    }
    Ok(())
}

/// The name and size of one requested file, captured before dispatch so the
/// readback can be compared against what was actually on disk.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct ExpectedFile {
    pub(crate) name: String,
    pub(crate) size: u64,
}

/// Every path must name an existing regular file on this host before the
/// backend is asked to read it; the names and sizes seen here are what the
/// readback must reproduce.
pub(crate) fn local_file_facts(paths: &[PathBuf]) -> Result<Vec<ExpectedFile>, BrowserControlFailure> {
    paths
        .iter()
        .map(|path| match std::fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => Ok(ExpectedFile {
                name: path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                size: metadata.len(),
            }),
            Ok(_) => Err(BrowserControlFailure::new(
                "not_a_file",
                format!("{} is not a regular file", path.display()),
            )),
            Err(error) => Err(BrowserControlFailure::new(
                "file_not_found",
                format!("{} cannot be read: {error}", path.display()),
            )),
        })
        .collect()
}

/// The input must hold exactly the requested files afterwards, as a multiset
/// of (name, size): a driver that accepted the command without attaching
/// anything, dropped a duplicate, or a handler that swapped in a file of a
/// different name or size is a failed action. Content of the same name and
/// size is not distinguished.
pub(crate) fn verify_attached(
    attached: &[BrowserAttachedFile],
    expected: &[ExpectedFile],
) -> Result<(), BrowserControlFailure> {
    let mut actual = attached
        .iter()
        .map(|file| ExpectedFile {
            name: file.name.clone(),
            size: file.size,
        })
        .collect::<Vec<_>>();
    actual.sort();
    let mut wanted = expected.to_vec();
    wanted.sort();
    if actual == wanted {
        Ok(())
    } else {
        Err(BrowserControlFailure::new(
            "attachment_mismatch",
            format!(
                "the file input holds {} file(s) after the attachment that do not match the {} requested by name and size",
                attached.len(),
                expected.len()
            ),
        ))
    }
}

/// Whether an HTML `accept` list admits `path`, judged by its name: `.ext`
/// tokens (including compound ones such as `.tar.gz`) match the end of the
/// file name, `type/subtype` and `type/*` tokens match the MIME type derived
/// from the final extension. A file whose extension is unknown is refused
/// by a MIME-only list rather than guessed. Tokens that are neither an
/// extension nor a MIME type are ignored, as browsers ignore them, so a
/// list with no valid token restricts nothing.
pub(crate) fn accept_allows(accept: &str, path: &Path) -> bool {
    let tokens = accept
        .split(',')
        .map(|token| token.trim().to_ascii_lowercase())
        .filter(|token| is_accept_token(token))
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return true;
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase());
    let mime = extension.as_deref().and_then(mime_for_extension);
    tokens.iter().any(|token| {
        if token == "*/*" {
            true
        } else if token.starts_with('.') {
            name.ends_with(token.as_str())
        } else if let Some(family) = token.strip_suffix("/*") {
            mime.is_some_and(|mime| mime.split('/').next() == Some(family))
        } else {
            mime == Some(token.as_str())
        }
    })
}

/// A valid file type specifier: an extension with something after the dot,
/// or `type/subtype` (`subtype` may be `*`) with both halves present.
fn is_accept_token(token: &str) -> bool {
    if let Some(extension) = token.strip_prefix('.') {
        return !extension.is_empty() && !extension.contains('/');
    }
    matches!(token.split_once('/'), Some((kind, subtype)) if is_mime_token(kind) && is_mime_token(subtype))
}

fn is_mime_token(token: &str) -> bool {
    !token.is_empty()
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
}

/// The MIME type an extension implies, from the shared registry with a few
/// image formats it does not carry.
fn mime_for_extension(extension: &str) -> Option<&'static str> {
    if let Some(mime) = mime_guess::from_ext(extension).first_raw() {
        return Some(mime);
    }
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
        "avif" => "image/avif",
        "jxl" => "image/jxl",
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

const RESET_FILE_INPUT_FUNCTION: &str = r"function(selector) {
    let element;
    try { element = document.querySelector(selector); }
    catch (error) { return { error: { code: 'invalid_selector', message: String(error?.message || error).slice(0, 512) } }; }
    if (!(element instanceof HTMLInputElement) || element.type !== 'file')
        return { error: { code: 'no_such_element', message: 'the file input is no longer in the document' } };
    if (element.files && element.files.length > 0) element.value = '';
    return { cleared: true };
}";

const FILE_INPUT_PROBE_FUNCTION: &str = r"function(selector) {
    let element;
    try { element = document.querySelector(selector); }
    catch (error) { return { error: { code: 'invalid_selector', message: String(error?.message || error).slice(0, 512) } }; }
    if (!element) return { error: { code: 'no_such_element', message: 'no element matched the target' } };
    if (!(element instanceof HTMLInputElement) || element.type !== 'file')
        return { error: { code: 'not_file_input', message: 'target element is not an input[type=file]; target the file input itself, not the button that opens the chooser' } };
    if (element.matches(':disabled') || element.getAttribute('aria-disabled') === 'true')
        return { error: { code: 'element_disabled', message: 'target element is disabled' } };
    const accept = String(element.getAttribute('accept') || '');
    return { accept: accept.slice(0, 8192), acceptLength: accept.length, multiple: element.hasAttribute('multiple') };
}";

// The readback also reads the first and last byte of every file: a browser
// can list a file it was handed and still be unable to open it (a Snap
// confined browser and a hidden directory, for instance), and that must be
// a failure here rather than an empty upload later.
const ATTACHED_FILES_FUNCTION: &str = r"function(selector) {
    let element;
    try { element = document.querySelector(selector); }
    catch (error) { return { error: { code: 'invalid_selector', message: String(error?.message || error).slice(0, 512) } }; }
    if (!(element instanceof HTMLInputElement) || element.type !== 'file')
        return { error: { code: 'no_such_element', message: 'the file input is no longer in the document' } };
    const files = Array.from(element.files || []);
    // Every file is really read, an empty one in full (an empty buffer)
    // and any other at its first and last byte, so a confined browser that
    // lists a file it cannot open fails here.
    const readable = (file) => file.size === 0
        ? file.arrayBuffer()
        : Promise.all([file.slice(0, 1).arrayBuffer(), file.slice(file.size - 1, file.size).arrayBuffer()]);
    return Promise.all(files.map((file) => readable(file).then(() => null, (error) => String(error?.message || error).slice(0, 256))))
        .then((failures) => {
            const failed = failures.findIndex((failure) => failure !== null);
            if (failed >= 0) return { error: { code: 'attachment_unreadable', message: `the browser cannot read ${String(files[failed].name).slice(0, 512)}: ${failures[failed]}` } };
            return { files: files.map((file) => ({
                name: String(file.name).slice(0, 512), size: Number(file.size) || 0, mime: String(file.type).slice(0, 128)
            })) };
        });
}";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn probe(accept: &str, multiple: bool) -> FileInputProbe {
        FileInputProbe {
            accept: accept.to_string(),
            accept_length: accept.chars().count(),
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
        assert!(
            accept_allows("garbage", pdf),
            "an invalid token restricts nothing, as in browsers"
        );
        assert!(accept_allows(".", pdf), "a bare dot is not a specifier");
        assert!(
            accept_allows("image/", pdf),
            "a type without a subtype is not a specifier"
        );
        assert!(
            !accept_allows("garbage, image/*", pdf),
            "valid tokens still apply beside invalid ones"
        );
        assert!(accept_allows("image/*", Path::new("/uploads/photo.avif")));
        assert!(accept_allows("audio/*", Path::new("/uploads/voice.flac")));
        assert!(accept_allows(
            "application/vnd.oasis.opendocument.presentation",
            Path::new("/uploads/deck.odp")
        ));
        assert!(!accept_allows("image/*", Path::new("/uploads/photo.unknownext")));
        assert!(accept_allows(".unknownext", Path::new("/uploads/photo.unknownext")));
        assert!(!accept_allows(".pdf", Path::new("/uploads/no-extension")));
        assert!(accept_allows(".tar.gz", Path::new("/uploads/archive.TAR.GZ")));
        assert!(!accept_allows(".tar.gz", Path::new("/uploads/archive.gz")));
        assert!(
            accept_allows(".env", Path::new("/uploads/.env")),
            "a dotfile named like the token matches"
        );
        assert!(accept_allows("application/gzip", Path::new("/uploads/archive.tar.gz")));
    }

    #[test]
    fn invalid_mime_parameters_and_syntax_do_not_restrict_the_picker() {
        for accept in [
            "image/png;charset=utf-8",
            "image /png",
            "image/png/extra",
            "image/π",
            "image/png=foo",
        ] {
            assert!(accept_allows(accept, Path::new("/uploads/a.png")), "{accept}");
            assert!(accept_allows(accept, Path::new("/uploads/a.txt")), "{accept}");
            assert!(!accept_allows(&format!("{accept},.pdf"), Path::new("/uploads/a.png")));
        }
        assert!(is_accept_token("application/vnd.example+json"));
        assert!(is_accept_token("image/*"));
        assert!(accept_allows("*/*", Path::new("/uploads/unknown.ext")));
        assert!(accept_allows("*/*,.pdf", Path::new("/uploads/a.png")));
        assert!(accept_allows("*/*", Path::new("/uploads/no_extension")));
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
        let oversized = FileInputProbe {
            accept: ".pdf".into(),
            accept_length: MAX_ACCEPT_CHARACTERS + 1,
            multiple: true,
        };
        assert_eq!(
            check_attachment_request(&oversized, &one).map_err(|error| error.code),
            Err("accept_unsupported".to_string()),
            "a list the probe could not carry in full is refused, never judged in part"
        );
        assert!(
            error.message.contains("b.png") && error.message.contains(".pdf"),
            "{}",
            error.message
        );
    }

    #[test]
    fn probes_and_readbacks_parse_or_surface_the_page_error() {
        let probe =
            parse_file_input_probe(&json!({ "accept": ".pdf", "acceptLength": 4, "multiple": true })).expect("probe");
        assert_eq!(
            probe,
            FileInputProbe {
                accept: ".pdf".into(),
                accept_length: 4,
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
    fn readback_must_hold_exactly_the_requested_files_by_name_and_size() {
        let expected = |files: &[(&str, u64)]| {
            files
                .iter()
                .map(|(name, size)| ExpectedFile {
                    name: (*name).to_string(),
                    size: *size,
                })
                .collect::<Vec<_>>()
        };
        let attached = |files: &[(&str, u64)]| {
            files
                .iter()
                .map(|(name, size)| BrowserAttachedFile {
                    name: (*name).to_string(),
                    size: *size,
                    mime: String::new(),
                })
                .collect::<Vec<_>>()
        };
        let wanted = expected(&[("a.pdf", 10), ("b.png", 20), ("a.pdf", 10)]);
        assert!(verify_attached(&attached(&[("b.png", 20), ("a.pdf", 10), ("a.pdf", 10)]), &wanted).is_ok());
        for retained in [
            &[("a.pdf", 10), ("b.png", 20)][..],
            &[("a.pdf", 10), ("b.png", 20), ("other.pdf", 10)],
            &[("a.pdf", 10), ("b.png", 21), ("a.pdf", 10)],
            &[],
        ] {
            let error = verify_attached(&attached(retained), &wanted).expect_err("mismatch");
            assert_eq!(error.code, "attachment_mismatch");
        }
    }

    #[test]
    fn local_files_must_exist_as_regular_files() {
        let directory = tempfile::tempdir().expect("tempdir");
        let file = directory.path().join("doc.pdf");
        std::fs::write(&file, b"%PDF-1.4").expect("write");
        assert_eq!(
            local_file_facts(std::slice::from_ref(&file)).expect("facts"),
            vec![ExpectedFile {
                name: "doc.pdf".into(),
                size: 8
            }]
        );
        assert_eq!(
            local_file_facts(&[directory.path().to_path_buf()]).map_err(|error| error.code),
            Err("not_a_file".to_string())
        );
        assert_eq!(
            local_file_facts(&[file, directory.path().join("missing.pdf")]).map_err(|error| error.code),
            Err("file_not_found".to_string())
        );
    }

    #[test]
    fn page_expressions_quote_the_selector() {
        assert!(file_input_probe_expression("input[name=\"doc\"]").contains("(\"input[name=\\\"doc\\\"]\")"));
        assert!(attached_files_expression("#doc").ends_with("(\"#doc\")"));
        assert!(reset_file_input_expression("#doc").ends_with("(\"#doc\")"));
        assert_eq!(element_handle_expression("#doc"), "document.querySelector(\"#doc\")");
    }
}
