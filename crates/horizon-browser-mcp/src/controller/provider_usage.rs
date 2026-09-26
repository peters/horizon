use super::BrowserController;
use horizon_browser_control::manifest::provider_usage;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProviderUsageInput {
    /// Configured provider profile name. Omit to query all configured providers.
    pub provider: Option<String>,
}

impl BrowserController {
    pub(crate) async fn provider_usage(&self, provider: Option<String>) -> Result<Value, String> {
        let id = provider_usage::enqueue_provider_usage(self.identity(), provider).map_err(|error| {
            match error.kind() {
                std::io::ErrorKind::PermissionDenied => {
                    "provider usage requires a live Horizon host identity and a configured provider name"
                }
                _ => "could not queue provider usage request",
            }
            .to_string()
        })?;
        let started = Instant::now();
        loop {
            if let Some(result) = provider_usage::take_provider_usage_result(self.identity(), &id)
                .map_err(|_| "could not read provider usage result".to_string())?
            {
                if let Some(error) = result.error {
                    return Err(error);
                }
                return Ok(json!({"providers": result.providers}));
            }
            if started.elapsed() >= Duration::from_secs(20) {
                return Err("provider usage timed out; session admission is unaffected".into());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProviderDevicesInput {
    /// Configured provider account. Credentials and endpoint are resolved by the host.
    pub provider: String,
    /// Optional words matching device, OS, OS version or browser (for example "iPhone 18").
    #[serde(default)]
    pub search: String,
    /// Offset returned by the previous page. Each page contains at most 50 combinations.
    #[serde(default)]
    pub offset: usize,
}
impl BrowserController {
    pub(crate) async fn provider_devices(&self, input: ProviderDevicesInput) -> Result<Value, String> {
        let query = horizon_browser::provider_catalog::CatalogQuery {
            provider: input.provider,
            search: input.search,
            offset: input.offset,
        };
        let id = provider_usage::enqueue_catalog(self.identity(), query).map_err(|_| {
            "provider_catalog_invalid_request: discovery requires a live host and valid provider query".to_string()
        })?;
        let started = Instant::now();
        loop {
            if let Some(result) = provider_usage::take_provider_usage_result(self.identity(), &id)
                .map_err(|_| "provider_catalog_result_unavailable".to_string())?
            {
                if let Some(error) = result.error {
                    return Err(error);
                }
                return Ok(json!({"catalog":result.catalog, "capacity_reserved":false}));
            }
            if started.elapsed() >= Duration::from_secs(20) {
                return Err("provider_catalog_timed_out".into());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

/// Requirements for `cloud_offers`. Every field is optional.
#[derive(Debug, Deserialize, serde::Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CloudOffersInput {
    /// Minimum vCPUs.
    pub min_vcpu: Option<u16>,
    /// Minimum memory in GB.
    pub min_memory_gb: Option<u16>,
    /// GPU workers instead of CPU workers.
    #[serde(default)]
    pub gpu: bool,
    /// Minimum GPU memory in GB.
    pub min_gpu_memory_gb: Option<u16>,
    /// A GPU type ID or name, such as "NVIDIA RTX A5000" or "RTX A5000".
    pub gpu_type: Option<String>,
    /// Highest acceptable hourly price in US dollars.
    pub max_hourly: Option<f64>,
    /// Expected running hours, for the estimated total. One hour when omitted.
    pub hours: Option<f64>,
    /// Workspace storage in GB priced into the estimate. 20 when omitted.
    pub storage_gb: Option<u16>,
    /// A region such as "EUROPE" or "North America".
    pub region: Option<String>,
    /// Also list GPU types without stock where the worker may go.
    #[serde(default)]
    pub include_unavailable: bool,
    /// At most this many offers, cheapest first: 1 to 50, and 10 when omitted.
    pub limit: Option<usize>,
}

impl BrowserController {
    pub(crate) async fn cloud_offers(&self, input: CloudOffersInput) -> Result<Value, String> {
        let requirements = serde_json::to_value(&input).map_err(|_| "cloud_offers_invalid_request".to_string())?;
        let id = provider_usage::enqueue_cloud_offers(self.identity(), requirements).map_err(|error| {
            match error.kind() {
                std::io::ErrorKind::PermissionDenied => "cloud_offers_unavailable: requires a Horizon agent panel",
                std::io::ErrorKind::InvalidInput => "cloud_offers_invalid_request",
                _ => "cloud_offers_unavailable: could not queue the request",
            }
            .to_string()
        })?;
        let started = Instant::now();
        loop {
            if let Some(result) = provider_usage::take_provider_usage_result(self.identity(), &id)
                .map_err(|_| "cloud_offers_result_unavailable".to_string())?
            {
                if let Some(error) = result.error {
                    return Err(error);
                }
                return result
                    .offers
                    .ok_or_else(|| "cloud_offers_result_unavailable".to_string());
            }
            // The host may fetch prices first, which takes a few seconds.
            if started.elapsed() >= Duration::from_secs(25) {
                return Err("cloud_offers_timed_out: Horizon did not answer; is it running?".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}
