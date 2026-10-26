//! Browser VM requests for the running Horizon. Deployment, stop, resume and delete
//! answer once Horizon recorded the phase it started; the caller polls `status`.
use super::BrowserController;
use horizon_browser_control::manifest::provider_usage::{
    self, BrowserVmAction, BrowserVmRequest, REQUEST_DEADLINE_MILLIS,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::time::{Duration, Instant};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BrowserVmInput {
    /// `status` reads the recorded phase and the SSH target and touches nothing.
    /// `deploy` rents the machine, `stop` releases the server and keeps the state
    /// volume, `resume` rents a new server on that volume, and `delete` removes both.
    pub operation: BrowserVmAction,
    /// An immutable image reference of the browser VM image,
    /// `[registry/]name@sha256:<64 hex>`. Required by `deploy`, and refused with a tag.
    pub image: Option<String>,
    /// A dedicated `ccx` server type, such as `ccx13`. Required by `deploy`.
    pub server_type: Option<String>,
    /// A location from the Hetzner settings, such as `fsn1`; `deploy` uses the first
    /// allowed one when this is omitted.
    pub location: Option<String>,
    /// Size of the state volume in GB; `deploy` uses the cloud default when omitted.
    pub volume_gb: Option<u16>,
}

/// The request's deadline plus a moment for Horizon's answer to be read.
const WAIT: Duration = Duration::from_millis(REQUEST_DEADLINE_MILLIS.unsigned_abs() + 1_000);

impl BrowserController {
    pub(crate) async fn browser_vm(&self, input: BrowserVmInput) -> Result<Value, String> {
        let request = BrowserVmRequest {
            action: input.operation,
            image: input.image,
            server_type: input.server_type,
            location: input.location,
            volume_gb: input.volume_gb,
        };
        let id = provider_usage::enqueue_browser_vm(self.identity(), request).map_err(|error| {
            match error.kind() {
                std::io::ErrorKind::PermissionDenied => "browser_vm_unavailable: requires a Horizon agent panel",
                std::io::ErrorKind::InvalidInput => {
                    "browser_vm_invalid_request: deploy needs an image with a digest and a ccx server type; the other operations take no machine"
                }
                _ => "browser_vm_unavailable: could not queue the request",
            }
            .to_string()
        })?;
        let started = Instant::now();
        loop {
            if let Some(result) = self
                .take_host_answer(&id)
                .map_err(|_| "browser_vm_result_unavailable".to_string())?
            {
                if let Some(error) = result.error {
                    return Err(error);
                }
                return result
                    .browser_vm
                    .ok_or_else(|| "browser_vm_result_unavailable".to_string());
            }
            if started.elapsed() >= WAIT {
                return Err("browser_vm_timed_out: Horizon did not answer; is it running?".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_input_refuses_unknown_fields_and_keeps_the_operation_required() {
        assert!(serde_json::from_value::<BrowserVmInput>(json!({"operation": "status"})).is_ok());
        assert!(serde_json::from_value::<BrowserVmInput>(json!({"operation": "status", "token": "x"})).is_err());
        assert!(serde_json::from_value::<BrowserVmInput>(json!({"image": "x"})).is_err());
        let deploy = serde_json::from_value::<BrowserVmInput>(json!({
            "operation": "deploy", "image": format!("ghcr.io/example/vm@sha256:{}", "a".repeat(64)),
            "server_type": "ccx13", "volume_gb": 20,
        }))
        .unwrap();
        assert_eq!(deploy.operation, BrowserVmAction::Deploy);
        assert!(deploy.location.is_none());
    }
}
