use super::super::network_volume::ApiNetworkVolume;
use super::{RunPodError, RunPodHttp, decode_json, valid_provider_id};

const OPERATION: &str = "network volume inspection";

impl RunPodHttp {
    pub(super) fn get_network_volume(&self, volume_id: &str) -> Result<Option<ApiNetworkVolume>, RunPodError> {
        if !valid_provider_id(volume_id) {
            return Err(RunPodError::InvalidTarget);
        }
        let response = self
            .agent
            .get(format!("https://api.runpod.io/v2/network-volumes/{volume_id}"))
            .header("Authorization", &self.authorization)
            .call()
            .map_err(|_| RunPodError::RequestFailed { operation: OPERATION })?;
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        decode_json(response, 200, OPERATION).map(Some)
    }
}
