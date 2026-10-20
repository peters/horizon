//! The PNG of a `browser_screenshot` as an MCP image block, for a client that
//! runs on another computer than the host (`horizon-browser mcp --connect
//! --inline-screenshots`, for example behind a relay). The host writes the
//! PNG under its runtime root, where only a process on the same computer can
//! read it, so this process adds its pixels to the result.

use std::io::Read as _;
use std::path::Path;

use base64::Engine as _;
use rmcp::model::{CallToolResponse, ContentBlock};

/// The largest PNG that goes into a result. Its base64 text stays below the
/// 16 MiB line limit of a typical relay.
pub const MAX_INLINE_BYTES: u64 = 8 * 1024 * 1024;

/// Add the PNG that a successful screenshot result names to its content when
/// the file is a `.png` under `root` and at most [`MAX_INLINE_BYTES`]. Any
/// other result stays as it is; its path still names the private file.
#[must_use]
pub fn inline(response: CallToolResponse, root: &Path) -> CallToolResponse {
    let CallToolResponse::Complete(mut result) = response else {
        return response;
    };
    if result.is_error != Some(true) {
        let path = result
            .structured_content
            .as_ref()
            .and_then(|value| value.get("path"))
            .and_then(serde_json::Value::as_str)
            .map(std::path::PathBuf::from);
        match path.as_deref().map(|path| read(path, root)) {
            Some(Ok(bytes)) => result.content.push(ContentBlock::image(
                base64::engine::general_purpose::STANDARD.encode(bytes),
                "image/png",
            )),
            Some(Err(reason)) => tracing::warn!(%reason, "the screenshot stays a path only"),
            None => {}
        }
    }
    CallToolResponse::Complete(result)
}

fn read(path: &Path, root: &Path) -> Result<Vec<u8>, String> {
    let path = path.canonicalize().map_err(|error| error.to_string())?;
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    if !path.starts_with(&root) || path.extension().is_none_or(|extension| extension != "png") {
        return Err("the capture is not a PNG under the runtime root".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&path)
        .and_then(|file| file.take(MAX_INLINE_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|error| error.to_string())?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_INLINE_BYTES {
        return Err(format!("the capture is larger than {MAX_INLINE_BYTES} bytes"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
