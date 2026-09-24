//! Provider-neutral environment descriptions. REST adapters belong here;
//! panel layout, repository preparation and agent execution do not.
#![forbid(unsafe_code)]

mod capabilities;
pub mod companions;
mod profile;
mod reason;
mod startup;
pub use capabilities::{Agent, BrowserEngine, BrowserStack, Capabilities};
pub use reason::Reason;
pub use startup::StartupMetadata;
pub mod runpod;
mod worker;
pub use profile::{
    Bootstrap, Build, CloudConfig, DESIGN_EXAMPLE, EXAMPLE, Profile, ProfileError, Storage, valid_id, valid_image,
};
pub use worker::*;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Environment {
    pub id: String,
    pub image: String,
    pub connection: Connection,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Connection {
    /// No provider calls, billing, SSH endpoint or remote-lifetime guarantee.
    LocalPrototype,
    ManagedWorker,
}

impl Environment {
    #[must_use]
    pub fn prototype(id: String) -> Self {
        Self {
            id,
            image: "development-image (mock)".into(),
            connection: Connection::LocalPrototype,
            profile: None,
            provider: None,
        }
    }
}
