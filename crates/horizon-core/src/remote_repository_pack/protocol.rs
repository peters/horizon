use super::{RemotePackExpectation, RemotePackInspectionError as Error, RemotePackObservation};
use crate::{
    cloud_run::{ArtifactDigest, GitCommitSha},
    repository_overlay::{
        checkout::publication::MAX_SIBLING_NAME_BYTES,
        seed::{MAX_OBJECTS, MAX_PACK_PATH_BYTES, receive::PackReceiveLimits},
    },
};
use serde::{Deserialize, Serialize};

const VERSION: u32 = 1;
const REQUEST_LIMIT: usize = (6 * MAX_PACK_PATH_BYTES + 4096).next_power_of_two();
pub(super) const RESPONSE_LIMIT: usize = (2 * 6 * (MAX_PACK_PATH_BYTES + 16) + 4096).next_power_of_two();

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    base_commit: GitCommitSha,
    sha256: ArtifactDigest,
    encoded_bytes: u64,
}

#[derive(Serialize)]
struct Request<'a> {
    version: u32,
    path: &'a str,
    pack: Identity,
}

pub(super) fn request(expected: RemotePackExpectation<'_>) -> Result<Vec<u8>, Error> {
    let path = expected.path;
    if path.len() > MAX_PACK_PATH_BYTES
        || !path.starts_with('/')
        || path.contains('\0')
        || path[1..]
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | "..") || part.len() > MAX_SIBLING_NAME_BYTES)
        || expected.base_commit.as_str().bytes().all(|byte| byte == b'0')
        || !(32..=PackReceiveLimits::default().encoded_bytes).contains(&expected.encoded_bytes)
    {
        return Err(Error::InvalidRequest);
    }
    let bytes = serde_json::to_vec(&Request {
        version: VERSION,
        path,
        pack: Identity {
            base_commit: expected.base_commit.clone(),
            sha256: expected.sha256.clone(),
            encoded_bytes: expected.encoded_bytes,
        },
    })
    .map_err(|_| Error::InvalidRequest)?;
    if bytes.len() > REQUEST_LIMIT {
        return Err(Error::InvalidRequest);
    }
    Ok(bytes)
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Observed,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    version: u32,
    status: Status,
    pack: Pack,
    retained: (),
    reason: (),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pack {
    path: String,
    objects_directory: String,
    identity: Identity,
    objects: u32,
}

pub(super) fn response(bytes: &[u8], expected: RemotePackExpectation<'_>) -> Result<RemotePackObservation, Error> {
    if bytes.len() > RESPONSE_LIMIT {
        return Err(Error::InvalidResponse);
    }
    let Response {
        version,
        status: Status::Observed,
        pack,
        retained: (),
        reason: (),
    } = serde_json::from_slice(bytes).map_err(|_| Error::InvalidResponse)?;
    if version != VERSION
        || pack.path != expected.path
        || pack.objects_directory != format!("{}/decoded/objects", expected.path)
        || pack.identity.base_commit != *expected.base_commit
        || pack.identity.sha256 != *expected.sha256
        || pack.identity.encoded_bytes != expected.encoded_bytes
        || pack.objects == 0
        || u64::from(pack.objects) > MAX_OBJECTS as u64
    {
        return Err(Error::InvalidResponse);
    }
    Ok(RemotePackObservation {
        path: pack.path,
        objects_directory: pack.objects_directory,
        base_commit: pack.identity.base_commit,
        sha256: pack.identity.sha256,
        encoded_bytes: pack.identity.encoded_bytes,
        objects: pack.objects,
    })
}
