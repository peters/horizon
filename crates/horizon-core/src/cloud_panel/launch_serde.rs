//! Fence reviewed publisher choices before a deployment record exists.
use super::{CloudLaunch, Placement};
use crate::cloud_runtime::publishing::Selection;
use serde::{Deserialize, Deserializer, Serialize, de::IgnoredAny};

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
pub(super) enum Wire {
    Reviewed(Reviewed),
    Legacy(Body),
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
            // The engine integration must forward the retained choice before accepting this envelope.
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
        if body.reserved.version
            || body.reserved.launch
            || matches!(&body.publishing_selection, Some(Selection::Recipe { image }) if image != &body.profile.image)
        {
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
