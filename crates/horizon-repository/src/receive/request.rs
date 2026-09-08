use super::{Command, VERSION};
use horizon_core::{
    cloud_run::ArtifactDigest,
    repository_overlay::{
        bundle::{RepositoryOverlayBundle, codec},
        materialize::MAX_REQUEST_PATH_BYTES,
    },
};
use serde::Deserialize;
use std::{
    io::{self, Read},
    path::{Component, PathBuf},
};

// One maximum escaped path, digest and fixed framing fields; no payload in JSON.
pub(super) const HEADER_LIMIT: usize = (6 * MAX_REQUEST_PATH_BYTES + 4096).next_power_of_two();

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservationRequest {
    version: u32,
    bundle_store: PathBuf,
    bundle_manifest: ArtifactDigest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiveHeader {
    version: u32,
    bundle_store: PathBuf,
    bundle_manifest: ArtifactDigest,
    encoded_bytes: usize,
}

pub(super) struct Request {
    pub(super) bundle_store: PathBuf,
    pub(super) bundle_manifest: ArtifactDigest,
}

impl Request {
    fn new(version: u32, bundle_store: PathBuf, bundle_manifest: ArtifactDigest) -> Result<Self, ()> {
        if version != VERSION
            || !bundle_store.is_absolute()
            || bundle_store.parent().is_none()
            || bundle_store.components().any(|part| part == Component::ParentDir)
            || !bundle_store
                .to_str()
                .is_some_and(|path| path.len() <= MAX_REQUEST_PATH_BYTES && !path.contains('\0'))
        {
            return Err(());
        }
        Ok(Self {
            bundle_store,
            bundle_manifest,
        })
    }
}

pub(super) enum Operation {
    Receive {
        request: Request,
        bundle: Box<RepositoryOverlayBundle>,
    },
    Observe(Request),
}

impl Operation {
    pub(super) fn request(&self) -> &Request {
        match self {
            Self::Receive { request, .. } | Self::Observe(request) => request,
        }
    }
}

pub(super) fn read(command: Command, input: &mut impl Read) -> Result<Operation, ()> {
    match command {
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
            Request::new(wire.version, wire.bundle_store, wire.bundle_manifest).map(Operation::Observe)
        }
        Command::Receive => receive(input),
    }
}

fn receive(input: &mut impl Read) -> Result<Operation, ()> {
    let mut prefix = [0; 4];
    input.read_exact(&mut prefix).map_err(|_| ())?;
    let length = usize::try_from(u32::from_le_bytes(prefix)).map_err(|_| ())?;
    if length == 0 || length > HEADER_LIMIT {
        return Err(());
    }
    let header = read_exact_bytes(input, length)?;
    let wire: ReceiveHeader = serde_json::from_slice(&header).map_err(|_| ())?;
    let request = Request::new(wire.version, wire.bundle_store, wire.bundle_manifest)?;
    if wire.encoded_bytes == 0 || wire.encoded_bytes > codec::MAX_ENCODED_BUNDLE_BYTES {
        return Err(());
    }
    let encoded = read_exact_bytes(input, wire.encoded_bytes)?;
    require_eof(input)?;
    let bundle = codec::decode(&encoded).map_err(|_| ())?;
    if bundle.manifest_sha256() != &request.bundle_manifest {
        return Err(());
    }
    // The input buffer drops before storage re-encodes/verifies any existing record.
    Ok(Operation::Receive {
        request,
        bundle: Box::new(bundle),
    })
}

fn read_exact_bytes(input: &mut impl Read, length: usize) -> Result<Vec<u8>, ()> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length).map_err(|_| ())?;
    bytes.resize(length, 0);
    input.read_exact(&mut bytes).map_err(|_| ())?;
    Ok(bytes)
}

fn require_eof(input: &mut impl Read) -> Result<(), ()> {
    let mut extra = [0];
    loop {
        match input.read(&mut extra) {
            Ok(0) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            _ => return Err(()),
        }
    }
}
