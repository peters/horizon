use super::{VERSION, request::Identity};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Status {
    Acknowledged,
    Observed,
    Rejected,
    Error,
    Unsupported,
    ReceiveUnconfirmed,
    Unpublished,
    PublishedUnsynchronized,
    RenameUnconfirmed,
}

#[derive(Serialize)]
struct VerifiedPack {
    path: PathBuf,
    objects_directory: PathBuf,
    identity: Identity,
    objects: u32,
}

#[derive(Serialize)]
struct Retained {
    source: Option<PathBuf>,
    destination: Option<PathBuf>,
}

#[derive(Serialize)]
pub(super) struct Response {
    version: u32,
    status: Status,
    pack: Option<VerifiedPack>,
    retained: Option<Retained>,
    reason: Option<&'static str>,
}

impl Response {
    pub(super) fn new(status: Status, reason: &'static str) -> Self {
        Self {
            version: VERSION,
            status,
            pack: None,
            retained: None,
            reason: Some(reason),
        }
    }

    pub(super) fn exit_code(&self) -> u8 {
        match self.status {
            Status::Acknowledged | Status::Observed => 0,
            Status::Rejected => 2,
            _ => 1,
        }
    }

    #[cfg(target_os = "linux")]
    pub(super) fn retained(
        status: Status,
        source: Option<PathBuf>,
        destination: Option<PathBuf>,
        reason: &'static str,
    ) -> Self {
        Self {
            retained: Some(Retained { source, destination }),
            ..Self::new(status, reason)
        }
    }

    #[cfg(target_os = "linux")]
    pub(super) fn verified(
        status: Status,
        pack: &horizon_core::repository_overlay::seed::receive::ReceivedGitPack,
        identity: &Identity,
    ) -> Self {
        Self {
            version: VERSION,
            status,
            pack: Some(VerifiedPack {
                path: pack.path().to_owned(),
                objects_directory: pack.objects_directory().to_owned(),
                identity: identity.clone(),
                objects: pack.objects(),
            }),
            retained: None,
            reason: None,
        }
    }
}
