use super::BindingError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! identity {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
        #[serde(try_from = "Uuid", into = "Uuid")]
        pub struct $name(Uuid);

        impl $name {
            /// Generate once and persist before publishing any related intent.
            #[must_use]
            pub fn generate() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl TryFrom<Uuid> for $name {
            type Error = BindingError;

            fn try_from(value: Uuid) -> Result<Self, Self::Error> {
                if value.is_nil() {
                    Err(BindingError::Identity)
                } else {
                    Ok(Self(value))
                }
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

identity!(
    AllocationId,
    "Stable allocation identity, independent of member clouds and provider IDs."
);
identity!(
    ProjectId,
    "Stable project namespace identity; titles and repository paths never select it."
);
identity!(
    ControllerId,
    "Identity of the single provider controller; copying it is not ownership transfer."
);
identity!(
    OperationId,
    "Persisted management operation identity, reused only for the same request."
);

/// Immutable saved-session membership, separate from an allocation reference.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "Membership", into = "Membership")]
pub struct ProjectIdentity(Membership);

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Membership {
    #[serde(rename = "project_id")]
    project: ProjectId,
    #[serde(rename = "session_id")]
    session: String,
    #[serde(rename = "workspace_id")]
    workspace: String,
    #[serde(rename = "cloud_id")]
    cloud: String,
}

impl ProjectIdentity {
    /// # Errors
    /// Rejects missing or non-portable persisted membership components.
    pub fn new(
        project_id: ProjectId,
        session_id: String,
        workspace_id: String,
        cloud_id: String,
    ) -> Result<Self, BindingError> {
        Membership {
            project: project_id,
            session: session_id,
            workspace: workspace_id,
            cloud: cloud_id,
        }
        .try_into()
    }

    #[must_use]
    pub const fn project_id(&self) -> ProjectId {
        self.0.project
    }

    #[must_use]
    pub fn cloud_id(&self) -> &str {
        &self.0.cloud
    }

    /// A copied workspace or session must not adopt another project's ownership.
    #[must_use]
    pub fn belongs_to(&self, session_id: &str, workspace_id: &str, cloud_id: &str) -> bool {
        self.0.session == session_id && self.0.workspace == workspace_id && self.0.cloud == cloud_id
    }
}

impl TryFrom<Membership> for ProjectIdentity {
    type Error = BindingError;

    fn try_from(value: Membership) -> Result<Self, Self::Error> {
        if [&value.session, &value.workspace, &value.cloud]
            .into_iter()
            .all(|component| horizon_cloud::valid_id(component))
        {
            Ok(Self(value))
        } else {
            Err(BindingError::Membership)
        }
    }
}

impl From<ProjectIdentity> for Membership {
    fn from(value: ProjectIdentity) -> Self {
        value.0
    }
}
