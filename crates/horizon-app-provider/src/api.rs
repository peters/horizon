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
    /// Restore only a privately journaled owned reference, never an agent-supplied token.
    /// # Errors
    /// Rejects malformed provider references without disclosing their contents.
    pub fn from_owned_reference(reference: &str) -> Result<Self> {
        Self::from_response(&serde_json::json!({"app_url":reference}))
    }

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
    pub(crate) authorization: Zeroizing<String>,
    pub(crate) hub: String,
    pub(crate) agent: ureq::Agent,
}

pub(crate) struct Decoded {
    pub(crate) value: Value,
    pub(crate) bytes: usize,
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
        self.upload_with_timeout(artifact, operation, Duration::from_secs(180))
    }

    /// # Errors
    /// The host's remaining monotonic lifetime bounds the complete upload request.
    pub fn upload_with_timeout(
        &self,
        artifact: &mut Artifact,
        operation: uuid::Uuid,
        timeout: Duration,
    ) -> Result<UploadedApp> {
        if timeout.is_zero() || timeout > Duration::from_secs(180) {
            return Err(Error::UploadUncertain);
        }
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
            .config()
            .timeout_global(Some(timeout))
            .build()
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
            .config()
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .call()
            .map_err(|_| Error::ProviderFailed)?;
        upload_delete_acknowledgement(response)
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

    /// # Errors
    /// Worker/binary/checksum/state are trusted host configuration, never project or MCP input.
    /// The callback records the guardian operation before authorizing its tunnel child.
    pub fn guarded_tunnel(
        &self,
        request: crate::tunnel_guard::Request,
        journal: impl FnOnce(uuid::Uuid, u32) -> Result<()>,
    ) -> Result<crate::tunnel_guard::GuardedTunnel> {
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
        crate::tunnel_guard::GuardedTunnel::start(request, key, journal)
    }

    pub(crate) fn get(&self, path: &str) -> Result<Value> {
        self.get_bounded(path, Duration::from_secs(180), MAX_RESPONSE)
    }

    pub(crate) fn get_bounded(&self, path: &str, timeout: Duration, bytes: u64) -> Result<Value> {
        self.get_measured(path, timeout, bytes).map(|response| response.value)
    }

    pub(crate) fn get_measured(&self, path: &str, timeout: Duration, bytes: u64) -> Result<Decoded> {
        decode_measured(
            self.agent
                .get(&format!("{API}{path}"))
                .header("Authorization", self.authorization.as_str())
                .config()
                .timeout_global(Some(timeout))
                .build()
                .call()
                .map_err(|error| discovery_http_error(&error))?,
            bytes,
        )
    }
}

fn discovery_http_error(error: &ureq::Error) -> Error {
    match error {
        ureq::Error::BodyExceedsLimit(_) | ureq::Error::Timeout(_) => Error::ReconcileIncomplete,
        _ => Error::ProviderFailed,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UploadDeleted {
    success: bool,
}
fn upload_delete_acknowledgement(mut response: ureq::http::Response<ureq::Body>) -> Result<()> {
    if response.status().as_u16() != 200 {
        return Err(Error::ProviderFailed);
    }
    let bytes = response
        .body_mut()
        .with_config()
        .limit(1024)
        .read_to_vec()
        .map_err(|_| Error::ProviderFailed)?;
    upload_deleted(&bytes)
}
fn upload_deleted(bytes: &[u8]) -> Result<()> {
    if bytes.iter().copied().find(|byte| !b" \n\r\t".contains(byte)) != Some(b'{') {
        return Err(Error::ProviderFailed);
    }
    let response: UploadDeleted = serde_json::from_slice(bytes).map_err(|_| Error::ProviderFailed)?;
    if response.success {
        Ok(())
    } else {
        Err(Error::ProviderFailed)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Acknowledgement {
    value: (),
}

pub(crate) fn quit_acknowledgement(mut response: ureq::http::Response<ureq::Body>) -> Result<()> {
    let status = response.status().as_u16();
    let bytes = response
        .body_mut()
        .with_config()
        .limit(1024 * 1024)
        .read_to_vec()
        .map_err(|_| Error::ProviderFailed)?;
    if status == 204 && bytes.is_empty() {
        return Ok(());
    }
    if bytes.iter().copied().find(|byte| !b" \n\r\t".contains(byte)) != Some(b'{') {
        return Err(Error::ProviderFailed);
    }
    let acknowledgement: Acknowledgement = serde_json::from_slice(&bytes).map_err(|_| Error::ProviderFailed)?;
    let () = acknowledgement.value;
    if status == 200 {
        return Ok(());
    }
    Err(Error::ProviderFailed)
}

fn decode(response: ureq::http::Response<ureq::Body>) -> Result<Value> {
    decode_bounded(response, MAX_RESPONSE)
}

fn decode_bounded(response: ureq::http::Response<ureq::Body>, limit: u64) -> Result<Value> {
    decode_measured(response, limit).map(|response| response.value)
}

pub(crate) fn decode_measured(mut response: ureq::http::Response<ureq::Body>, limit: u64) -> Result<Decoded> {
    if !response.status().is_success() {
        return Err(Error::ProviderFailed);
    }
    let bytes = response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|error| discovery_http_error(&error))?;
    Ok(Decoded {
        value: serde_json::from_slice(&bytes).map_err(|_| Error::ProviderRejected)?,
        bytes: bytes.len(),
    })
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

#[cfg(test)]
mod acknowledgement_tests {
    use super::*;
    #[test]
    fn quit_requires_exact_positive_response() {
        for (status, body, confirmed) in [
            (200, r#"{"value":null}"#, true),
            (204, "", true),
            (200, "{}", false),
            (200, "[null]", false),
            (200, r#"{"value":{"error":"failed"},"value":null}"#, false),
            (200, r#"{"value":null,"value":null}"#, false),
            (200, "null", false),
            (200, r#"{"value":{"error":"unexpected"}}"#, false),
            (200, r#"{"value":null,"status":13}"#, false),
            (201, r#"{"value":null}"#, false),
            (204, "{}", false),
            (500, r#"{"value":null}"#, false),
        ] {
            let response = ureq::http::Response::builder()
                .status(status)
                .body(ureq::Body::builder().data(body.as_bytes().to_vec()))
                .unwrap();
            assert_eq!(quit_acknowledgement(response).is_ok(), confirmed, "{status} {body}");
        }
    }
    #[test]
    fn discovery_limits_keep_their_typed_exhaustion() {
        assert_eq!(
            discovery_http_error(&ureq::Error::Timeout(ureq::Timeout::Global)),
            Error::ReconcileIncomplete
        );
        assert_eq!(
            discovery_http_error(&ureq::Error::BodyExceedsLimit(12)),
            Error::ReconcileIncomplete
        );
        assert_eq!(
            discovery_http_error(&ureq::Error::ConnectionFailed),
            Error::ProviderFailed
        );
    }
    #[test]
    fn upload_delete_requires_one_unambiguous_positive_object() {
        upload_deleted(br#"{"success":true}"#).unwrap();
        for invalid in [
            br#"{"success":false}"#.as_slice(),
            br#"{"success":false,"success":true}"#,
            br#"{"success":true,"error":"contradictory"}"#,
            br"{}",
            br"[true]",
            b"null",
            br#"{"success":null}"#,
        ] {
            assert_eq!(upload_deleted(invalid), Err(Error::ProviderFailed));
        }
    }
}
