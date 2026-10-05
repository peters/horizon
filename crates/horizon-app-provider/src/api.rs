use std::io::{Cursor, Read};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use horizon_app_testing::catalog::{Device, decode as decode_catalog};
use horizon_browser::{ClassicTransport, RemoteAuthorizationHeader, RemoteHttpClient};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::Zeroizing;

use crate::artifact::Artifact;
use crate::{Error, Result};

const API: &str = "https://api-cloud.browserstack.com";
const MAX_RESPONSE: u64 = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(from = "ProviderQuota")]
pub struct Quota {
    pub parallel_sessions_max_allowed: u32,
    pub team_parallel_sessions_max_allowed: u32,
    pub parallel_sessions_running: u32,
    pub queued_sessions: u32,
}

#[derive(Deserialize)]
struct ProviderQuota {
    parallel_sessions_max_allowed: u32,
    team_parallel_sessions_max_allowed: Option<u32>,
    parallel_sessions_running: u32,
    queued_sessions: u32,
}

impl From<ProviderQuota> for Quota {
    fn from(value: ProviderQuota) -> Self {
        Self {
            parallel_sessions_max_allowed: value.parallel_sessions_max_allowed,
            team_parallel_sessions_max_allowed: value
                .team_parallel_sessions_max_allowed
                .unwrap_or(value.parallel_sessions_max_allowed),
            parallel_sessions_running: value.parallel_sessions_running,
            queued_sessions: value.queued_sessions,
        }
    }
}

impl Quota {
    #[must_use]
    pub fn available(self) -> u32 {
        self.parallel_sessions_max_allowed
            .min(self.team_parallel_sessions_max_allowed)
            .saturating_sub(self.parallel_sessions_running.saturating_add(self.queued_sessions))
    }
}

/// Raw provider app token: host-only, deliberately neither Debug nor Serialize.
pub struct UploadedApp {
    token: Zeroizing<String>,
}

impl UploadedApp {
    /// # Errors
    /// Only `BrowserStack`'s bounded hexadecimal app references are accepted.
    pub(crate) fn from_response(value: &Value) -> Result<Self> {
        let token = value
            .get("app_url")
            .and_then(Value::as_str)
            .ok_or(Error::ProviderRejected)?;
        let id = token.strip_prefix("bs://").ok_or(Error::ProviderRejected)?;
        if !(16..=128).contains(&id.len()) || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::ProviderRejected);
        }
        Ok(Self {
            token: Zeroizing::new(token.to_owned()),
        })
    }

    pub fn use_for_driver<T>(&self, host: impl FnOnce(&str) -> T) -> T {
        host(&self.token)
    }
}

/// Uses only the machine-configured `BrowserStack` origin; project files cannot select a credential destination.
pub struct BrowserStack {
    authorization: Zeroizing<String>,
    hub: String,
    agent: ureq::Agent,
}

impl BrowserStack {
    /// # Errors
    /// Requires a trusted `BrowserStack` hub origin and printable Basic authorization from the host resolver.
    pub fn new(origin: &str, authorization: Zeroizing<String>) -> Result<Self> {
        if !horizon_browser::provider_usage::UsageAdapter::Browserstack.authorizes_origin(origin) {
            return Err(Error::ProviderRejected);
        }
        let encoded = authorization.strip_prefix("Basic ").ok_or(Error::ProviderRejected)?;
        let decoded = Zeroizing::new(
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|_| Error::ProviderRejected)?,
        );
        let text = std::str::from_utf8(&decoded).map_err(|_| Error::ProviderRejected)?;
        let (user, key) = text.split_once(':').ok_or(Error::ProviderRejected)?;
        if user.is_empty()
            || key.is_empty()
            || key.contains(':')
            || text.len() > 4096
            || !text.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(Error::ProviderRejected);
        }
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(180)))
            .timeout_connect(Some(Duration::from_secs(10)))
            .build();
        Ok(Self {
            authorization,
            hub: origin.to_owned(),
            agent: ureq::Agent::new_with_config(config),
        })
    }

    /// # Errors
    /// Returns native App Automate capacity, never browser account capacity.
    pub fn quota(&self) -> Result<Quota> {
        serde_json::from_value(self.get("/app-automate/plan.json")?).map_err(|_| Error::ProviderRejected)
    }

    /// # Errors
    /// Only verified physical native catalog rows can resolve a matrix.
    pub fn devices(&self) -> Result<Vec<Device>> {
        let value = self.get("/app-automate/devices.json")?;
        decode_catalog(&serde_json::to_vec(&value).map_err(|_| Error::ProviderRejected)?)
            .map_err(|_| Error::ProviderRejected)
    }

    /// # Errors
    /// The host must journal its operation before this call; uncertain uploads must not be replayed.
    pub fn upload(&self, artifact: &mut Artifact, operation: uuid::Uuid) -> Result<UploadedApp> {
        let boundary = format!("horizon-{}", uuid::Uuid::new_v4().simple());
        let extension = match artifact.platform() {
            horizon_app_testing::contract::Platform::Ios => "ipa",
            horizon_app_testing::contract::Platform::Android => "apk",
        };
        let prefix = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"custom_id\"\r\n\r\nhorizon-native-{operation}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"app.{extension}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        );
        let suffix = format!("\r\n--{boundary}--\r\n");
        let length = artifact.bytes() + prefix.len() as u64 + suffix.len() as u64;
        let mut reader = Cursor::new(prefix).chain(artifact.reader()?).chain(Cursor::new(suffix));
        let response = self
            .agent
            .post(&format!("{API}/app-automate/upload"))
            .header("Authorization", self.authorization.as_str())
            .header("Content-Type", format!("multipart/form-data; boundary={boundary}"))
            .header("Content-Length", length.to_string())
            .send(ureq::SendBody::from_reader(&mut reader))
            .map_err(|_| Error::UploadUncertain)?;
        UploadedApp::from_response(&decode(response).map_err(|_| Error::UploadUncertain)?)
            .map_err(|_| Error::UploadUncertain)
    }

    /// # Errors
    /// Deletes only the app represented by this private owned token; confirmation is required.
    pub fn delete_app(&self, app: &UploadedApp) -> Result<()> {
        let id = app.token.strip_prefix("bs://").ok_or(Error::ProviderRejected)?;
        let response = self
            .agent
            .delete(&format!("{API}/app-automate/app/delete/{id}"))
            .header("Authorization", self.authorization.as_str())
            .call()
            .map_err(|_| Error::ProviderFailed)?;
        if decode(response)?.get("success").and_then(Value::as_bool) != Some(true) {
            return Err(Error::ProviderFailed);
        }
        Ok(())
    }

    /// # Errors
    /// Builds the shared native driver transport without exposing the authorization header.
    pub fn driver(&self) -> Result<Arc<dyn ClassicTransport>> {
        let authorization =
            RemoteAuthorizationHeader::new(self.authorization.to_string()).map_err(|_| Error::ProviderRejected)?;
        Ok(Arc::new(
            RemoteHttpClient::new(&format!("{}/wd/hub", self.hub), Some(authorization))
                .map_err(|_| Error::ProviderRejected)?,
        ))
    }

    /// # Errors
    /// Starts a bounded restricted tunnel. The access key never reaches argv, environment or public outputs.
    pub fn tunnel(
        &self,
        binary: crate::tunnel::VerifiedBinary,
        ports: Vec<crate::tunnel::LocalPort>,
        id: uuid::Uuid,
        lifetime: Duration,
        journal: impl Fn(crate::tunnel::ProcessRecord<'_>) -> Result<()>,
    ) -> Result<crate::tunnel::Tunnel> {
        let encoded = self
            .authorization
            .strip_prefix("Basic ")
            .ok_or(Error::ProviderRejected)?;
        let decoded = Zeroizing::new(
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|_| Error::ProviderRejected)?,
        );
        let text = std::str::from_utf8(&decoded).map_err(|_| Error::ProviderRejected)?;
        let (_, key) = text.split_once(':').ok_or(Error::ProviderRejected)?;
        crate::tunnel::Tunnel::start(binary, key, ports, id, lifetime, journal)
    }

    pub(crate) fn get(&self, path: &str) -> Result<Value> {
        self.get_bounded(path, Duration::from_secs(180), MAX_RESPONSE)
    }

    pub(crate) fn get_bounded(&self, path: &str, timeout: Duration, bytes: u64) -> Result<Value> {
        decode_bounded(
            self.agent
                .get(&format!("{API}{path}"))
                .header("Authorization", self.authorization.as_str())
                .config()
                .timeout_global(Some(timeout))
                .build()
                .call()
                .map_err(|_| Error::ProviderFailed)?,
            bytes,
        )
    }
}

fn decode(response: ureq::http::Response<ureq::Body>) -> Result<Value> {
    decode_bounded(response, MAX_RESPONSE)
}

fn decode_bounded(mut response: ureq::http::Response<ureq::Body>, limit: u64) -> Result<Value> {
    if !response.status().is_success() {
        return Err(Error::ProviderFailed);
    }
    let bytes = response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|_| Error::ProviderFailed)?;
    serde_json::from_slice(&bytes).map_err(|_| Error::ProviderRejected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn native_quota_accounts_for_shared_team_use_and_queue() {
        for team in [None, Some(2)] {
            let mut response =
                json!({"parallel_sessions_max_allowed":4,"parallel_sessions_running":1,"queued_sessions":0});
            if let Some(team) = team {
                response["team_parallel_sessions_max_allowed"] = json!(team);
            }
            let decoded: Quota = serde_json::from_value(response).unwrap();
            assert_eq!(decoded.available(), if team.is_some() { 1 } else { 3 });
        }
        let quota = Quota {
            parallel_sessions_max_allowed: 4,
            team_parallel_sessions_max_allowed: 2,
            parallel_sessions_running: 1,
            queued_sessions: 0,
        };
        assert_eq!(quota.available(), 1);
        assert_eq!(
            Quota {
                queued_sessions: 1,
                ..quota
            }
            .available(),
            0
        );
        assert_eq!(
            Quota {
                parallel_sessions_running: u32::MAX,
                queued_sessions: u32::MAX,
                ..quota
            }
            .available(),
            0
        );
    }

    #[test]
    fn credentials_cannot_be_bound_to_project_selected_origins() {
        for origin in [
            "https://example.com",
            "http://hub-cloud.browserstack.com",
            "https://hub-cloud.browserstack.com:443",
            "https://api-cloud.browserstack.com",
        ] {
            let auth = Zeroizing::new(format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode("synthetic:synthetic")
            ));
            assert_eq!(BrowserStack::new(origin, auth).err(), Some(Error::ProviderRejected));
        }
    }

    #[test]
    fn configured_regional_hubs_remain_bound_to_the_driver() {
        for region in ["cloud", "apse", "aps", "euw", "use", "usw"] {
            let hub = format!("https://hub-{region}.browserstack.com");
            let auth = Zeroizing::new(format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode("synthetic:synthetic")
            ));
            let provider = BrowserStack::new(&hub, auth).unwrap();
            assert_eq!(provider.hub, hub);
            assert!(provider.driver().is_ok());
        }
    }

    #[test]
    fn invalid_credentials_and_provider_app_references_return_only_constant_errors() {
        for auth in [
            "Bearer synthetic-secret",
            "Basic bad",
            "Basic dXNlcjo=",
            "Basic OnNlY3JldA==",
        ] {
            assert_eq!(
                BrowserStack::new("https://hub-cloud.browserstack.com", Zeroizing::new(auth.into())).err(),
                Some(Error::ProviderRejected)
            );
        }
        for value in [
            json!({"app_url":"https://example.com/private-token"}),
            json!({"app_url":"bs://bad"}),
            json!({"app_url":"bs://../../private-token"}),
        ] {
            assert_eq!(UploadedApp::from_response(&value).err(), Some(Error::ProviderRejected));
        }
        let app =
            UploadedApp::from_response(&json!({"app_url":"bs://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"})).unwrap();
        assert_eq!(app.use_for_driver(str::len), 45);
    }
}
