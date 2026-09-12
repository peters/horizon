//! VM power actions (deallocate, start): one POST to the exact VM resource, the
//! long-running acceptance mapped, an absent VM reported as `None`.
use super::{AzureArmHttp, AzureError, AzureLongRunningState, COMPUTE_API_VERSION, long_running};

impl AzureArmHttp {
    /// POST `action` (a fixed path suffix such as `/start`) on the named VM.
    pub(super) fn power_operation(
        &self,
        group: &str,
        name: &str,
        action: &'static str,
        operation: &'static str,
    ) -> Result<Option<AzureLongRunningState>, AzureError> {
        let url = self.resource_url(
            group,
            "Microsoft.Compute/virtualMachines",
            name,
            COMPUTE_API_VERSION,
            action,
        )?;
        match self.send_json("POST", &url, None, &[200, 202], false, operation) {
            Ok((status, _)) => Ok(Some(long_running(status))),
            Err(AzureError::UnexpectedStatus { status: 404, .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }
}
