use horizon_app_testing::catalog::Device;
use horizon_app_testing::contract::Platform;
use serde::Serialize;
use uuid::Uuid;

#[derive(Clone, Serialize)]
pub struct Evidence {
    pub id: Uuid,
    pub path: Option<std::path::PathBuf>,
    pub bytes: usize,
    pub state: EvidenceState,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceState {
    Available,
    Expired,
    Unavailable,
}
#[derive(Serialize)]
pub struct Step {
    pub recipe: String,
    pub step: String,
    pub passed: bool,
    pub duration_millis: u64,
    pub error: Option<String>,
    pub screenshot: Option<Evidence>,
}
#[derive(Clone, Copy)]
pub enum CaptureKind {
    Screenshot,
    Provider(horizon_app_provider::media::Kind),
}
#[derive(Serialize)]
pub struct Media {
    pub session: Uuid,
    pub kind: horizon_app_provider::media::Kind,
    pub evidence: Option<Evidence>,
    pub error: Option<String>,
}
#[derive(Serialize)]
pub struct DeviceResult {
    pub matrix_index: usize,
    pub target: Device,
    pub session: Option<Uuid>,
    pub allocations: Vec<Uuid>,
    pub error: Option<String>,
    pub cleanup_confirmed: bool,
    pub steps: Vec<Step>,
    pub media: Vec<Media>,
    pub provider_session_link: Option<String>,
    pub provider_link_error: Option<String>,
}
#[derive(Serialize)]
pub struct Build {
    pub platform: Platform,
    pub duration_millis: u64,
    pub error: Option<String>,
}
#[derive(Serialize)]
pub struct Report {
    pub id: Uuid,
    pub parallel: usize,
    pub builds: Vec<Build>,
    pub devices: Vec<DeviceResult>,
    pub cancelled: bool,
    pub upload_cleanup_errors: Vec<String>,
}
#[derive(Clone, Serialize)]
pub struct Progress {
    pub run: Uuid,
    pub matrix_index: Option<usize>,
    pub phase: &'static str,
    pub recipe: Option<String>,
    pub step: Option<String>,
    pub session: Option<Uuid>,
    pub view: Option<crate::view::Handle>,
}
