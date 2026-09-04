//! Compact checkpoint metadata backed by immutable per-step result artifacts.

use std::path::Path;

use crate::{
    StepReport,
    checkpoint::{CheckpointCompletion, ResumeError},
};

use super::{RunState, RunStateError, io_error, sync_directory, write_private_json, write_private_json_once};

pub(super) const DIRECTORY: &str = "checkpoints";

#[derive(Clone, Debug)]
pub(super) struct PendingCompletion {
    pub(super) index: usize,
    pub(super) report: StepReport,
}

pub(super) fn metadata(index: usize, report: &StepReport) -> CheckpointCompletion {
    CheckpointCompletion {
        id: report.id.clone(),
        tool: report.tool.clone(),
        report_file: relative_path(index, &report.id),
    }
}

pub(super) fn persist_snapshot(
    directory: &Path,
    state_path: &Path,
    state: &RunState,
    pending: Option<&PendingCompletion>,
) -> Result<(), RunStateError> {
    if let Some(pending) = pending {
        persist_completion(directory, state, pending)?;
    }
    write_private_json(state_path, state, "state")?;
    sync_directory(directory)
}

pub(super) fn load(directory: &Path, completed: &[CheckpointCompletion]) -> Result<Vec<StepReport>, ResumeError> {
    completed
        .iter()
        .enumerate()
        .map(|(index, metadata)| load_completion(directory, index, metadata))
        .collect()
}

fn persist_completion(directory: &Path, state: &RunState, pending: &PendingCompletion) -> Result<(), RunStateError> {
    let Some(saved) = state.checkpoint.completed.get(pending.index) else {
        return Err(invalid_checkpoint("pending completion is missing from state"));
    };
    let expected = metadata(pending.index, &pending.report);
    if saved != &expected {
        return Err(invalid_checkpoint("pending completion does not match state"));
    }
    let path = directory.join(&saved.report_file);
    match write_private_json_once(&path, &pending.report, "checkpoint completion") {
        Ok(()) => {}
        Err(RunStateError::Io { source, .. }) if source.kind() == std::io::ErrorKind::AlreadyExists => {
            verify_existing(&path, &pending.report)?;
        }
        Err(error) => return Err(error),
    }
    sync_directory(&directory.join(DIRECTORY))
}

fn load_completion(directory: &Path, index: usize, metadata: &CheckpointCompletion) -> Result<StepReport, ResumeError> {
    let expected_path = relative_path(index, &metadata.id);
    if metadata.report_file != expected_path {
        return Err(ResumeError::Decode(format!(
            "durable checkpoint `{}` has invalid result path `{}`",
            metadata.id, metadata.report_file
        )));
    }
    let path = directory.join(&metadata.report_file);
    let bytes = std::fs::read(&path)
        .map_err(|source| ResumeError::Decode(format!("could not read {}: {source}", path.display())))?;
    let report: StepReport = serde_json::from_slice(&bytes)
        .map_err(|source| ResumeError::Decode(format!("could not decode {}: {source}", path.display())))?;
    if report.id != metadata.id || report.tool != metadata.tool {
        return Err(ResumeError::Decode(format!(
            "durable checkpoint result {} does not match state metadata",
            path.display()
        )));
    }
    Ok(report)
}

fn verify_existing(path: &Path, expected: &StepReport) -> Result<(), RunStateError> {
    let saved = std::fs::read(path)
        .map_err(|source| io_error(format!("could not read existing {}", path.display()), source))?;
    let mut expected_bytes = serde_json::to_vec_pretty(expected).map_err(|source| RunStateError::Encode {
        artifact: "checkpoint completion",
        source,
    })?;
    expected_bytes.push(b'\n');
    if saved != expected_bytes {
        return Err(invalid_checkpoint(
            "checkpoint artifact already contains another result",
        ));
    }
    Ok(())
}

fn relative_path(index: usize, step_id: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded_id = String::with_capacity(step_id.len().saturating_mul(2));
    for byte in step_id.bytes() {
        encoded_id.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded_id.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    format!("{DIRECTORY}/{index:06}-{encoded_id}.json")
}

fn invalid_checkpoint(message: &str) -> RunStateError {
    io_error(
        message.to_string(),
        std::io::Error::from(std::io::ErrorKind::InvalidData),
    )
}
