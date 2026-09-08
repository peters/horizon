use super::super::{SCRATCH_NAME, SetupIntent, codec as claim};
use super::snapshot::CompletionData;
use super::{SetupCompletion, SetupCompletionState as State, SetupRecordError as Error};
use crate::{cloud_run::ArtifactDigest, repository_overlay::checkout::publication::validate_sibling_name};
use serde::{Deserialize, Serialize};
use std::path::Path;

// Three escaped retained child paths plus canonical identity and a bounded reason.
pub(in super::super) const MAX_BYTES: usize = 128 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    claim_sha256: ArtifactDigest,
    completion: CompletionData,
}

pub(in super::super) fn encode(
    root: &Path,
    intent: &SetupIntent,
    completion: &SetupCompletion,
) -> Result<Vec<u8>, Error> {
    validate(root, intent, &completion.data)?;
    let record = Record {
        version: 1,
        claim_sha256: ArtifactDigest::sha256(&claim::encode(intent)?),
        completion: completion.data.clone(),
    };
    let bytes = serde_json::to_vec(&record).map_err(|_| Error::InvalidRecord)?;
    if bytes.len() > MAX_BYTES {
        return Err(Error::InvalidRecord);
    }
    Ok(bytes)
}

pub(in super::super) fn decode(root: &Path, intent: &SetupIntent, bytes: &[u8]) -> Result<SetupCompletion, Error> {
    if bytes.len() > MAX_BYTES {
        return Err(Error::InvalidRecord);
    }
    let record: Record = serde_json::from_slice(bytes).map_err(|_| Error::InvalidRecord)?;
    let completion = SetupCompletion {
        data: record.completion,
    };
    if record.version != 1
        || record.claim_sha256 != ArtifactDigest::sha256(&claim::encode(intent)?)
        || encode(root, intent, &completion)? != bytes
    {
        return Err(Error::InvalidRecord);
    }
    Ok(completion)
}

fn validate(root: &Path, intent: &SetupIntent, value: &CompletionData) -> Result<(), Error> {
    let scratch = root.join(SCRATCH_NAME);
    for path in [&value.source_metadata, &value.checkout, &value.possible_destination]
        .into_iter()
        .flatten()
    {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(Error::InvalidRecord)?;
        if path.parent() != Some(scratch.as_path()) || validate_sibling_name(name).is_err() {
            return Err(Error::InvalidRecord);
        }
    }
    let identity = value.base_commit.is_some();
    if identity != value.bundle_manifest.is_some()
        || value
            .bundle_manifest
            .as_ref()
            .is_some_and(|digest| digest != &intent.bundle_manifest)
        || value
            .base_commit
            .as_ref()
            .is_some_and(|base| !git2::Oid::from_str(base).is_ok_and(|oid| !oid.is_zero() && oid.to_string() == *base))
        || (value.checkout.is_some() && value.source_metadata.is_none())
        || (value.checkout.is_some() && value.checkout == value.source_metadata)
        || (identity && value.checkout.is_none())
        || value
            .reason
            .as_ref()
            .is_some_and(|reason| reason.is_empty() || reason.len() > 1024 || reason.contains('\0'))
    {
        return Err(Error::InvalidRecord);
    }
    let complete = value.source_metadata.is_some() && value.checkout.is_some() && identity;
    let destination = scratch.join(&intent.destination);
    let valid = match value.state {
        State::Rejected => {
            value.reason.is_some()
                && value.source_metadata.is_none()
                && value.checkout.is_none()
                && !identity
                && value.possible_destination.is_none()
        }
        State::Unpublished => value.reason.is_some() && value.possible_destination.is_none(),
        State::Published | State::PublishedUnsynchronized => {
            complete
                && value.checkout.as_ref() == Some(&destination)
                && value.possible_destination.is_none()
                && value.reason.is_none() == (value.state == State::Published)
        }
        State::RenameUnconfirmed => {
            complete
                && value.reason.is_some()
                && value.possible_destination.as_ref() == Some(&destination)
                && value.checkout != value.possible_destination
        }
    };
    if valid { Ok(()) } else { Err(Error::InvalidRecord) }
}
