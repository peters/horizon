//! Dedicated-worker SSH grant operations. These operations never manage compute.
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

pub const VERSION: u32 = 1;

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Identity {
        grant: String,
    },
    Authorize {
        grant: String,
        public_key: String,
        revision: String,
    },
    Connect {
        grant: String,
        alias: String,
        host: IpAddr,
        port: u16,
        host_key: String,
    },
    Revoke {
        grant: String,
    },
    Disconnect {
        grant: String,
    },
}

impl Request {
    #[must_use]
    pub fn grant(&self) -> &str {
        match self {
            Self::Identity { grant }
            | Self::Authorize { grant, .. }
            | Self::Connect { grant, .. }
            | Self::Revoke { grant }
            | Self::Disconnect { grant } => grant,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum Response {
    Identity { public_key: String },
    Authorized { host_key: String, worktree: String },
    Connected { ssh_alias: String, worktree: String },
    Revoked,
    Disconnected,
}

/// A safe discovery snapshot supplied by the owning controller; never an access grant.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub version: u32,
    pub source_cloud_id: String,
    pub observed_at: u64,
    pub companions: Vec<Companion>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Companion {
    pub alias: String,
    pub repository: String,
    pub profile: String,
    pub target_cloud_id: Option<String>,
    pub selected: bool,
    pub status: Status,
    pub access: Option<Access>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Unselected,
    Missing,
    Ambiguous,
    Stopped,
    Unavailable,
    Connecting,
    Ready,
    Unverified,
    Unreachable,
    Changed,
    RevocationPending,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Access {
    pub grant: String,
    pub ssh_alias: String,
    pub worktree: String,
}

impl Catalog {
    /// # Errors
    /// Rejects malformed discovery data before it is published or used in a command.
    pub fn validate(&self) -> Result<(), &'static str> {
        // Up to 64 declarations plus 64 retained grants awaiting cleanup after configuration changes.
        if self.version != VERSION || !horizon_cloud::valid_id(&self.source_cloud_id) || self.companions.len() > 128 {
            return Err("Invalid companion catalog");
        }
        let mut aliases = std::collections::BTreeSet::new();
        let mut grants = std::collections::BTreeSet::new();
        for entry in &self.companions {
            if !horizon_cloud::companions::valid_alias(&entry.alias)
                || !aliases.insert(&entry.alias)
                || (entry.selected && entry.target_cloud_id.is_none())
                || (horizon_cloud::companions::Declaration {
                    repository: entry.repository.clone(),
                    profile: entry.profile.clone(),
                })
                .validate()
                .is_err()
                || entry
                    .target_cloud_id
                    .as_deref()
                    .is_some_and(|id| !horizon_cloud::valid_id(id) || id == self.source_cloud_id)
            {
                return Err("Invalid companion identity in catalog");
            }
            if entry.status == Status::Ready && entry.access.is_none() {
                return Err("Ready companion requires connection details");
            }
            if let Some(access) = &entry.access
                && (!entry.selected
                    || entry.target_cloud_id.is_none()
                    || !horizon_cloud::valid_id(&access.grant)
                    || !grants.insert(&access.grant)
                    || access.ssh_alias != format!("companion-{}", entry.alias)
                    || access.worktree != format!("/workspace/companions/worktrees/{}", access.grant))
            {
                return Err("Invalid companion connection details");
            }
        }
        Ok(())
    }
}
