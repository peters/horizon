//! The bytes of one finished download that the caller named, added to the
//! answer of `browser_downloads`.
//!
//! A download is saved on the computer that runs the page. An agent that works
//! through a profile host of another computer (`horizon-browser mcp --connect
//! --remote <target>`) cannot open the path of that computer with its own
//! read-only tools, so this rewrite adds the bytes of the one file the caller
//! named. The bound keeps a line of the MCP stream within the same limit that an
//! inlined capture or screenshot uses.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use rmcp::model::{CallToolResponse, ContentBlock, ResourceContents};
use serde_json::Value;

/// Largest download that goes into an answer. Its base64 text stays below the
/// 16 MiB line limit of a typical relay.
pub const MAX_INLINE_BYTES: u64 = 8 * 1024 * 1024;

/// The structured field that states that the bytes did not cross, with the same
/// name and meaning as for a withheld capture of the remote bridge.
const WITHHELD_FIELD: &str = "inline_withheld";

/// Why the bytes of the named download are not in the answer.
#[derive(Debug, Eq, PartialEq)]
enum Refusal {
    /// No completed row of this answer has that file name.
    NotReported,
    /// The row names a file outside the panel folder.
    OutsideFolder,
    /// The file is larger than [`MAX_INLINE_BYTES`].
    TooLarge,
    /// The file could not be read.
    Unreadable,
}

impl Refusal {
    fn says(&self, file_name: &str) -> String {
        match self {
            Self::NotReported => format!(
                "no completed download named {file_name} is in this answer; read the rows and call again with a file name that has a path"
            ),
            Self::OutsideFolder => format!(
                "the reported path of {file_name} is outside the panel's download folder, so the bytes stay where they are"
            ),
            Self::TooLarge => format!(
                "the download {file_name} is larger than the {MAX_INLINE_BYTES} byte bound, so its bytes stay on the computer that saved them; read it there"
            ),
            Self::Unreadable => format!(
                "the download {file_name} could not be read, so its bytes stay on the computer that saved them; read it there"
            ),
        }
    }

    fn withholds(&self) -> bool {
        matches!(self, Self::TooLarge | Self::Unreadable)
    }
}

/// Add the bytes of the download that `file_name` names to a `browser_downloads`
/// answer. Any other answer, a named file that the answer does not report, or a
/// file that does not cross the bound keeps its rows and gains one text block
/// that states why.
#[must_use]
pub fn attach(response: CallToolResponse, file_name: &str) -> CallToolResponse {
    let CallToolResponse::Complete(mut result) = response else {
        return response;
    };
    if result.is_error == Some(true) {
        return CallToolResponse::Complete(result);
    }
    let Some(structured) = result.structured_content.clone() else {
        return CallToolResponse::Complete(result);
    };
    match found(&structured, file_name).and_then(|path| {
        let directory = structured.get("directory").and_then(Value::as_str).unwrap_or_default();
        resolve(directory, &path, file_name).and_then(|path| read(&path).map(|bytes| (path, bytes)))
    }) {
        Ok((path, bytes)) => {
            let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
            result.content.push(ContentBlock::resource(
                ResourceContents::blob(
                    base64::engine::general_purpose::STANDARD.encode(&bytes),
                    format!(
                        "horizon-browser://download/{}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ),
                )
                .with_mime_type("application/octet-stream"),
            ));
            if let Some(structured) = result.structured_content.as_mut().and_then(Value::as_object_mut) {
                structured.insert("fetched_file_name".to_string(), Value::String(file_name.to_string()));
                structured.insert("fetched_bytes".to_string(), Value::from(size));
            }
        }
        Err(refusal) => {
            tracing::info!(file_name, ?refusal, "the download bytes do not cross");
            if refusal.withholds()
                && let Some(structured) = result.structured_content.as_mut().and_then(Value::as_object_mut)
            {
                structured.insert(WITHHELD_FIELD.to_string(), Value::Bool(true));
            }
            result.content.push(ContentBlock::text(refusal.says(file_name)));
        }
    }
    CallToolResponse::Complete(result)
}

/// The path that the newest completed row of this answer reports for the name.
fn found(structured: &Value, file_name: &str) -> Result<String, Refusal> {
    let rows = structured
        .get("downloads")
        .and_then(Value::as_array)
        .ok_or(Refusal::NotReported)?;
    let row = rows
        .iter()
        .rev()
        .find(|row| {
            row.get("file_name").and_then(Value::as_str) == Some(file_name)
                && row.get("state").and_then(Value::as_str) == Some("completed")
        })
        .ok_or(Refusal::NotReported)?;
    row.get("path")
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .ok_or(Refusal::NotReported)
}

/// One file of the panel folder, never another one: the name is one path
/// component, and the canonical path of the file stays inside the canonical
/// folder, so a symlink or a reported path cannot reach outside it.
fn resolve(directory: &str, path: &str, file_name: &str) -> Result<PathBuf, Refusal> {
    let name = Path::new(file_name);
    if name.file_name() != Some(name.as_os_str()) || file_name == "." || file_name == ".." {
        return Err(Refusal::OutsideFolder);
    }
    let folder = Path::new(directory).canonicalize().map_err(|_| Refusal::Unreadable)?;
    let path = Path::new(path).canonicalize().map_err(|_| Refusal::Unreadable)?;
    if !path.starts_with(&folder) || path.file_name() != Some(name.as_os_str()) {
        return Err(Refusal::OutsideFolder);
    }
    Ok(path)
}

fn read(path: &Path) -> Result<Vec<u8>, Refusal> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(MAX_INLINE_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|_| Refusal::Unreadable)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_INLINE_BYTES {
        return Err(Refusal::TooLarge);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
