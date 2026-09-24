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
