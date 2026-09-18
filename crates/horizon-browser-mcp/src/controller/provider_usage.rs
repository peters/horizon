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
