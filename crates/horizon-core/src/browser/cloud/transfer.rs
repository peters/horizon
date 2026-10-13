//! Sends local files to the control service of a worker for one cloud browser.
//!
//! A transfer runs on its own thread and its own SSH connection, so the
//! presentation polls continue. The worker is not trusted: the host reads only
//! its receipts and errors, and shortens an error before the person sees it.
use base64::Engine;
use horizon_browser_protocol::cloud_view::{
    CloudStageChunk, CloudStagedFiles, CloudUploadLimits, CloudViewRequest, CloudViewResponse, MAX_UPLOAD_NAME_BYTES,
    valid_upload_name,
};
use sha2::{Digest, Sha256};
use std::{
    fmt::Write as _,
    io::{BufRead, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

/// The smallest part size that the host accepts from a worker.
const MIN_CHUNK_BYTES: u64 = 64 * 1024;
/// The most bytes of one worker response that the host reads.
const MAX_RESPONSE_BYTES: u64 = 64 * 1024;
/// The most characters of worker or page text that the host shows.
pub(super) const MAX_SHOWN_CHARS: usize = 300;

/// The files of one choice or drop, with their names on the worker.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Plan {
    files: Vec<(PathBuf, String, u64)>,
    pub(super) total: u64,
    chunk: u64,
}

#[derive(Debug, Default)]
pub(super) struct Progress {
    pub(super) sent: AtomicU64,
    pub(super) total: AtomicU64,
}

/// One request and its response on a connection to the control service.
pub(super) trait Link {
    fn call(&mut self, request: &CloudViewRequest) -> Result<CloudViewResponse, String>;
}

/// A `horizon-cloud-worker connect` process.
pub(super) struct StreamLink<W, R> {
    pub(super) input: W,
    pub(super) output: R,
}

impl<W: Write, R: BufRead> Link for StreamLink<W, R> {
    fn call(&mut self, request: &CloudViewRequest) -> Result<CloudViewResponse, String> {
        let lost = || "The connection to the cloud worker closed during the upload".to_owned();
        serde_json::to_writer(&mut self.input, request).map_err(|_| lost())?;
        self.input
            .write_all(b"\n")
            .and_then(|()| self.input.flush())
            .map_err(|_| lost())?;
        let mut line = String::new();
        let read = (&mut self.output)
            .take(MAX_RESPONSE_BYTES + 1)
            .read_line(&mut line)
            .map_err(|_| lost())?;
        if read == 0 || read as u64 > MAX_RESPONSE_BYTES {
            return Err(lost());
        }
        serde_json::from_str(&line).map_err(|_| "The cloud worker sent an invalid upload reply".to_owned())
    }
}

/// The host limits, lowered by the limits of the worker.
pub(super) fn effective_limits(worker: CloudUploadLimits) -> Result<CloudUploadLimits, String> {
    let host = CloudUploadLimits::default();
    if worker.max_files == 0 || worker.max_batch_bytes == 0 || worker.max_chunk_bytes < MIN_CHUNK_BYTES {
        return Err("The cloud worker reports invalid upload limits".into());
    }
    Ok(CloudUploadLimits {
        max_files: host.max_files.min(worker.max_files),
        max_batch_bytes: host.max_batch_bytes.min(worker.max_batch_bytes),
        max_chunk_bytes: host.max_chunk_bytes.min(worker.max_chunk_bytes),
    })
}

/// Reads the sizes and names of the files. Call it on the transfer thread.
pub(super) fn plan(paths: &[PathBuf], limits: CloudUploadLimits) -> Result<Plan, String> {
    if paths.is_empty() || paths.len() > limits.max_files {
        return Err(format!("Choose 1 to {} files", limits.max_files));
    }
    let mut files = Vec::with_capacity(paths.len());
    let mut total = 0_u64;
    for path in paths {
        let metadata = std::fs::metadata(path).map_err(|error| format!("Cannot read {}: {error}", path.display()))?;
        if metadata.is_dir() {
            return Err(format!(
                "{} is a folder. Drop the files in the folder, or an archive of it.",
                path.display()
            ));
        }
        if !metadata.is_file() {
            return Err(format!("{} is not a regular file", path.display()));
        }
        total = total.saturating_add(metadata.len());
        if total > limits.max_batch_bytes {
            return Err(format!(
                "The files are larger than the upload limit of {} MB",
                limits.max_batch_bytes / (1024 * 1024)
            ));
        }
        let name = unique_name(&files, &upload_name(path));
        files.push((path.clone(), name, metadata.len()));
    }
    Ok(Plan {
        files,
        total,
        chunk: limits.max_chunk_bytes,
    })
}

/// Sends every file of `plan` in order and returns the staged batch.
pub(super) fn send(
    plan: &Plan,
    browser: &str,
    batch: &str,
    link: &mut dyn Link,
    progress: &Progress,
    stop: &AtomicBool,
) -> Result<CloudStagedFiles, String> {
    progress.total.store(plan.total, Ordering::Relaxed);
    let chunk = usize::try_from(plan.chunk).unwrap_or(usize::MAX);
    let mut buffer = vec![0_u8; chunk];
    for (path, name, size) in &plan.files {
        let mut file = std::fs::File::open(path).map_err(|error| format!("Cannot read {}: {error}", path.display()))?;
        let mut digest = Sha256::new();
        let mut offset = 0_u64;
        loop {
            if stop.load(Ordering::Acquire) {
                return Err("The upload stopped".into());
            }
            let read = read_up_to(&mut file, &mut buffer).map_err(|error| format!("Cannot read {name}: {error}"))?;
            let end = offset + read as u64;
            if end > *size || (read < buffer.len() && end != *size) {
                return Err(format!("{name} changed during the upload. Upload it again."));
            }
            digest.update(&buffer[..read]);
            let last = end == *size;
            let response = link.call(&CloudViewRequest::Stage(CloudStageChunk {
                id: browser.to_owned(),
                batch: batch.to_owned(),
                name: name.clone(),
                size: *size,
                offset,
                data: base64::engine::general_purpose::STANDARD.encode(&buffer[..read]),
                sha256: last.then(|| hex(&digest.clone().finalize())),
            }))?;
            if let Some(error) = response.error {
                return Err(shorten(&error));
            }
            let receipt = response
                .staged
                .ok_or("The cloud worker does not accept uploads. Rebuild the cloud with a newer image.")?;
            if receipt.received != end || receipt.complete != last {
                return Err(format!(
                    "The cloud worker did not confirm all of {name}. Upload it again."
                ));
            }
            progress.sent.fetch_add(read as u64, Ordering::Relaxed);
            offset = end;
            if last {
                break;
            }
        }
    }
    Ok(CloudStagedFiles {
        batch: batch.to_owned(),
        names: plan.files.iter().map(|(_, name, _)| name.clone()).collect(),
    })
}

/// Worker or page text, shortened for display.
pub(super) fn shorten(text: &str) -> String {
    let mut shown: String = text.chars().filter(|c| !c.is_control()).take(MAX_SHOWN_CHARS).collect();
    if text.chars().count() > MAX_SHOWN_CHARS {
        shown.push('…');
    }
    shown
}

fn read_up_to(file: &mut impl Read, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..])? {
            0 => break,
            read => filled += read,
        }
    }
    Ok(filled)
}

/// The file name as one valid path component on the worker.
fn upload_name(path: &Path) -> String {
    let name: String = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_control() || c == '/' || c == '\\' {
                '_'
            } else {
                c
            }
        })
        .collect();
    let name = if name.len() > MAX_UPLOAD_NAME_BYTES {
        // Keep a short extension, so that the `accept` filter of the page still matches.
        let extension = name.rfind('.').map_or("", |dot| &name[dot..]);
        let extension = if extension.len() <= 32 { extension } else { "" };
        let stem = truncate_bytes(
            &name[..name.len() - extension.len()],
            MAX_UPLOAD_NAME_BYTES - extension.len(),
        );
        format!("{stem}{extension}")
    } else {
        name
    };
    if valid_upload_name(&name) { name } else { "file".into() }
}

/// Adds ` (2)`, ` (3)` and so on before the extension of a repeated name.
fn unique_name(files: &[(PathBuf, String, u64)], name: &str) -> String {
    let taken = |candidate: &str| files.iter().any(|(_, name, _)| name == candidate);
    if !taken(name) {
        return name.to_owned();
    }
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name, ""),
    };
    // One of these is free: the batch has fewer names than numbers.
    (2..=files.len() + 1)
        .map(|number| {
            // The marker always fits; a long extension shortens before the stem does.
            let marker = format!(" ({number})");
            let extension = truncate_bytes(extension, MAX_UPLOAD_NAME_BYTES - marker.len());
            let stem = truncate_bytes(stem, MAX_UPLOAD_NAME_BYTES - marker.len() - extension.len());
            format!("{stem}{marker}{extension}")
        })
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| name.to_owned())
}

fn truncate_bytes(text: &str, limit: usize) -> String {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::with_capacity(64), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

#[cfg(test)]
mod tests;
