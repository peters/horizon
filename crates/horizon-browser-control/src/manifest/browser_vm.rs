//! Agent requests for the browser VM: the one rented machine that runs the browser image
//! of issue 1780, with a state volume that keeps its profiles and their sign-ins.
//!
//! The host authorizes every field against its own cloud settings and provider policy.
//! Deployment, stop, resume and delete are long provider runs, so the request answers as
//! soon as the host recorded the phase it started, and the caller polls `status`.
use serde::{Deserialize, Serialize};

/// What an agent asks of the browser VM.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserVmAction {
    /// Rent the machine, or renew the request of one that broke before it was placed.
    Deploy,
    /// Read the recorded phase and the SSH target, without touching the provider.
    Status,
    /// Release the server and keep the state volume, so the sign-ins stay.
    Stop,
    /// Rent a new server on the kept state volume.
    Resume,
    /// Remove the server and the state volume.
    Delete,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BrowserVmRequest {
    pub action: BrowserVmAction,
    /// An immutable image reference, `[registry/]name@sha256:<64 hex>`, of the browser VM
    /// image. Required by `deploy`; a tag is refused, because a machine must not change
    /// what it runs between two rents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// A dedicated `ccx` server type, such as `ccx13`. Required by `deploy`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_type: Option<String>,
    /// A location the Hetzner settings allow, such as `fsn1`. `deploy` uses the first
    /// allowed location when it names none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// State volume size in GB. `deploy` uses the host default when it names none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_gb: Option<u16>,
}

impl BrowserVmRequest {
    /// Whether the fields this action needs are present and well formed. The host still
    /// authorizes the image, the location and the server type against its own settings.
    #[must_use]
    pub fn valid(&self) -> bool {
        let name = |value: &Option<String>, longest: usize| {
            value.as_ref().is_some_and(|value| {
                !value.is_empty()
                    && value.len() <= longest
                    && !value.starts_with('-')
                    && value
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | ':' | '@' | '='))
            })
        };
        match self.action {
            BrowserVmAction::Deploy => {
                name(&self.image, 512)
                    && self.image.as_deref().is_some_and(|image| image.contains("@sha256:"))
                    && name(&self.server_type, 64)
                    && self.location.as_ref().is_none_or(|_| name(&self.location, 64))
                    && self.volume_gb.is_none_or(|gb| (10..=10_240).contains(&gb))
            }
            BrowserVmAction::Status | BrowserVmAction::Stop | BrowserVmAction::Resume | BrowserVmAction::Delete => {
                self.image.is_none()
                    && self.server_type.is_none()
                    && self.location.is_none()
                    && self.volume_gb.is_none()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_a_deploy_names_the_machine_and_it_needs_a_digest() {
        let deploy = json!({"action": "deploy", "image": format!("ghcr.io/example/vm@sha256:{}", "a".repeat(64)),
            "server_type": "ccx13", "location": "fsn1", "volume_gb": 20});
        assert!(
            serde_json::from_value::<BrowserVmRequest>(deploy.clone())
                .unwrap()
                .valid()
        );
        let mut tagged = deploy.clone();
        tagged["image"] = "ghcr.io/example/vm:1780".into();
        assert!(!serde_json::from_value::<BrowserVmRequest>(tagged).unwrap().valid());
        for action in ["status", "stop", "resume", "delete"] {
            let mut other = deploy.clone();
            other["action"] = action.into();
            assert!(
                !serde_json::from_value::<BrowserVmRequest>(other).unwrap().valid(),
                "{action} takes no image"
            );
        }
        let mut small = deploy;
        small["volume_gb"] = 5.into();
        assert!(!serde_json::from_value::<BrowserVmRequest>(small).unwrap().valid());
    }

    #[test]
    fn a_status_read_takes_no_machine_and_unknown_fields_are_refused() {
        let status = serde_json::from_value::<BrowserVmRequest>(json!({"action": "status"})).unwrap();
        assert!(status.valid());
        assert!(serde_json::from_value::<BrowserVmRequest>(json!({"action": "status", "token": "x"})).is_err());
        assert!(serde_json::from_value::<BrowserVmRequest>(json!({})).is_err());
    }
}
