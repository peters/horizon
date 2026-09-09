use super::{Command, VERSION};
use horizon_core::{
    cloud_run::{ArtifactDigest, GitCommitSha},
    repository_overlay::{
        checkout::publication::validate_sibling_name,
        materialize::{MAX_REQUEST_PATH_BYTES, valid_path},
    },
};
use serde::{Deserialize, Serialize};
use std::{io::Read, path::PathBuf};

pub(super) const HEADER_LIMIT: usize = (6 * MAX_REQUEST_PATH_BYTES + 4096).next_power_of_two();
pub(super) use horizon_core::repository_overlay::seed::{
    MAX_PACK_PATH_BYTES, MAX_PACK_RECEIVE_PARENT_BYTES as MAX_RECEIVE_PARENT_BYTES,
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Identity {
    pub(super) base_commit: GitCommitSha,
    pub(super) sha256: ArtifactDigest,
    pub(super) encoded_bytes: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiveHeader {
    version: u32,
    parent: PathBuf,
    destination: String,
    pack: Identity,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservationRequest {
    version: u32,
    path: PathBuf,
    pack: Identity,
}

pub(super) struct Request {
    pub(super) path: PathBuf,
    pub(super) destination: Option<String>,
    pub(super) pack: Identity,
}

impl Request {
    fn new(version: u32, path: PathBuf, destination: Option<String>, pack: Identity) -> Result<Self, ()> {
        let request = Self {
            path,
            destination,
            pack,
        };
        if version != VERSION
            || !valid_path(&request.path)
            || request.path.parent().is_none()
            || request.path.to_str().is_none_or(|path| {
                path.len()
                    > if request.destination.is_some() {
                        MAX_RECEIVE_PARENT_BYTES
                    } else {
                        MAX_PACK_PATH_BYTES
                    }
            })
            || request
                .destination
                .as_deref()
                .is_some_and(|name| validate_sibling_name(name).is_err())
            || request.pack.base_commit.as_str().bytes().all(|byte| byte == b'0')
            || request.pack.encoded_bytes == 0
        {
            return Err(());
        }
        Ok(request)
    }
}

pub(super) fn read(command: Command, input: &mut impl Read) -> Result<Request, ()> {
    match command {
        Command::Receive => {
            let mut prefix = [0; 4];
            input.read_exact(&mut prefix).map_err(|_| ())?;
            let length = usize::try_from(u32::from_le_bytes(prefix)).map_err(|_| ())?;
            if length == 0 || length > HEADER_LIMIT {
                return Err(());
            }
            let mut bytes = Vec::new();
            bytes.try_reserve_exact(length).map_err(|_| ())?;
            input.take(length as u64).read_to_end(&mut bytes).map_err(|_| ())?;
            if bytes.len() != length {
                return Err(());
            }
            let wire: ReceiveHeader = serde_json::from_slice(&bytes).map_err(|_| ())?;
            Request::new(wire.version, wire.parent, Some(wire.destination), wire.pack)
        }
        Command::Observe => {
            let mut bytes = Vec::new();
            input
                .take(HEADER_LIMIT as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| ())?;
            if bytes.len() > HEADER_LIMIT {
                return Err(());
            }
            let wire: ObservationRequest = serde_json::from_slice(&bytes).map_err(|_| ())?;
            Request::new(wire.version, wire.path, None, wire.pack)
        }
    }
}
