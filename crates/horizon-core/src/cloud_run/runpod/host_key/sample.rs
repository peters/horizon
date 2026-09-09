//! A bounded sample of complete log events, not an exhaustive history or boot proof.

use super::super::{CloudJobId, CloudWorkflowId, RunPodError, RunPodWorker, http::RESPONSE_LIMIT_BYTES};
use crate::cloud_run::{CLOUD_RUN_PROTOCOL_VERSION, interactive_worker::valid_ssh_public_key};
use serde::Deserialize;
use std::io::{self, Read};

pub(super) const PREFIX: &str = "horizon-worker-host-key ";
const MAX_LINE_BYTES: usize = 16 * 1024;
const MAX_LINES: usize = 16_000;
const MAX_RECORD_BYTES: usize = 1024;

fn invalid() -> RunPodError {
    RunPodError::InvalidResponse {
        operation: "host-key bootstrap",
    }
}

pub(in super::super) fn read(reader: impl Read) -> Result<Vec<u8>, RunPodError> {
    let mut bytes = Vec::new();
    if let Err(error) = reader.take(RESPONSE_LIMIT_BYTES + 1).read_to_end(&mut bytes) {
        let timeout = error.kind() == io::ErrorKind::TimedOut
            || matches!(
                error.get_ref().and_then(|inner| inner.downcast_ref::<ureq::Error>()),
                Some(ureq::Error::Timeout(_))
            );
        if !timeout {
            return Err(invalid());
        }
    }
    if bytes.len() as u64 > RESPONSE_LIMIT_BYTES {
        return Err(invalid());
    }
    Ok(bytes)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Event {
    source: String,
    line: String,
    ts: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    pod_id: String,
    workflow_id: CloudWorkflowId,
    job_id: CloudJobId,
    cloud_protocol_version: u32,
    access_digest: String,
    host_public_key: String,
}

pub(super) fn parse(bytes: &[u8], worker: &RunPodWorker, digest: &str) -> Result<Option<String>, RunPodError> {
    if bytes.len() as u64 > RESPONSE_LIMIT_BYTES || (!bytes.is_empty() && !bytes.ends_with(b"\n")) {
        return Err(invalid());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    let mut data = None;
    let mut id = false;
    let mut selected = None;
    for (count, raw) in text.split_terminator('\n').enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if count >= MAX_LINES || line.len() > MAX_LINE_BYTES {
            return Err(invalid());
        }
        if line.is_empty() {
            if let Some(payload) = data.take() {
                accept(payload, worker, digest, &mut selected)?;
            }
            id = false;
        } else if let Some(value) = line.strip_prefix("data:") {
            if data.replace(value.strip_prefix(' ').unwrap_or(value)).is_some() {
                return Err(invalid());
            }
        } else if line.starts_with("id:") && !id {
            id = true;
        } else if !line.starts_with(':') {
            return Err(invalid());
        }
    }
    if data.is_some() || id {
        return Err(invalid());
    }
    Ok(selected)
}

fn accept(
    payload: &str,
    worker: &RunPodWorker,
    digest: &str,
    selected: &mut Option<String>,
) -> Result<(), RunPodError> {
    let event: Event = serde_json::from_str(payload).map_err(|_| invalid())?;
    if event.source != "container"
        || event.ts.len() > 64
        || time::OffsetDateTime::parse(&event.ts, &time::format_description::well_known::Rfc3339).is_err()
    {
        return Err(invalid());
    }
    let Some(record) = event.line.strip_prefix(PREFIX) else {
        return Ok(());
    };
    if event.line.len() > MAX_RECORD_BYTES {
        return Err(invalid());
    }
    let record: Record = serde_json::from_str(record).map_err(|_| invalid())?;
    if record.version != super::VERSION
        || record.pod_id != worker.pod_id
        || record.workflow_id != worker.workflow_id
        || record.job_id != worker.job_id
        || record.cloud_protocol_version != CLOUD_RUN_PROTOCOL_VERSION
        || record.access_digest != digest
        || !valid_ssh_public_key(&record.host_public_key)
        || record.host_public_key.split_ascii_whitespace().count() != 2
        || selected.as_ref().is_some_and(|key| *key != record.host_public_key)
    {
        return Err(invalid());
    }
    *selected = Some(record.host_public_key);
    Ok(())
}
