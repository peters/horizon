//! Fence reviewed publisher choices before a deployment record exists.
use super::{CloudLaunch, Placement};
use crate::cloud_runtime::publishing::Selection;
use serde::{Deserialize, Deserializer, Serialize, de::IgnoredAny};

#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum Wire {
    Reviewed(Reviewed),
    Legacy(Body),
}

impl Serialize for Wire {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let body = match self {
            Self::Reviewed(reviewed) => &reviewed.launch,
            Self::Legacy(body) => body,
        };
        if body.invalid_intent() {
            return Err(serde::ser::Error::custom("Invalid image launch confirmation"));
        }
        match self {
            Self::Reviewed(reviewed) => reviewed.serialize(serializer),
            Self::Legacy(body) => body.serialize(serializer),
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Reviewed {
    version: u32,
    launch: Body,
}

#[derive(Deserialize, Serialize)]
pub(super) struct Body {
    #[serde(flatten, skip_serializing, default)]
    reserved: ReservedEnvelope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    publishing_selection: Option<Selection>,
    #[serde(default)]
    deployment_started: bool,
    id: String,
    revision: String,
    profile_name: String,
    profile: horizon_cloud::Profile,
    #[serde(default, skip_serializing_if = "Placement::is_default")]
    placement: Placement,
}

impl Body {
    fn invalid_intent(&self) -> bool {
        match &self.publishing_selection {
            None => false,
            Some(Selection::Recipe { image }) => image != &self.profile.image,
            Some(Selection::Automatic { .. } | Selection::Registry { .. } | Selection::Personal { .. }) => {
                self.profile.build.is_none()
            }
        }
    }
}

#[derive(Default, Deserialize)]
struct ReservedEnvelope {
    #[serde(default, deserialize_with = "present")]
    version: bool,
    #[serde(default, deserialize_with = "present")]
    launch: bool,
}

fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    IgnoredAny::deserialize(deserializer)?;
    Ok(true)
}

impl From<CloudLaunch> for Wire {
    fn from(launch: CloudLaunch) -> Self {
        let CloudLaunch {
            publishing_selection,
            deployment_started,
            id,
            revision,
            profile_name,
            profile,
            placement,
        } = launch;
        let body = Body {
            reserved: ReservedEnvelope::default(),
            publishing_selection,
            deployment_started,
            id,
            revision,
            profile_name,
            profile,
            placement,
        };
        if matches!(
            body.publishing_selection.as_ref(),
            None | Some(Selection::Recipe { .. })
        ) {
            Self::Legacy(body)
        } else {
            Self::Reviewed(Reviewed {
                version: 2,
                launch: body,
            })
        }
    }
}

impl TryFrom<Wire> for CloudLaunch {
    type Error = &'static str;

    fn try_from(wire: Wire) -> Result<Self, Self::Error> {
        let body = match wire {
            Wire::Reviewed(Reviewed { version: 2, launch })
                if matches!(
                    launch.publishing_selection.as_ref(),
                    Some(Selection::Automatic { .. } | Selection::Registry { .. } | Selection::Personal { .. })
                ) =>
            {
                launch
            }
            Wire::Legacy(body)
                if matches!(
                    body.publishing_selection.as_ref(),
                    None | Some(Selection::Recipe { .. })
                ) =>
            {
                body
            }
            _ => return Err("Unsupported or incomplete reviewed image launch; use a compatible application"),
        };
        if body.reserved.version || body.reserved.launch || body.invalid_intent() {
            return Err("Invalid or downgraded image launch confirmation");
        }
        let Body {
            reserved: _,
            publishing_selection,
            deployment_started,
            id,
            revision,
            profile_name,
            profile,
            placement,
        } = body;
        Ok(Self {
            publishing_selection,
            deployment_started,
            id,
            revision,
            profile_name,
            profile,
            placement,
        })
    }
}

#[cfg(test)]
mod tests;
