use std::fmt::Debug;
use std::time::Duration;

/// Live Chromium endpoint and page metadata exported to an optional host
/// coordination layer. Horizon uses this boundary for its agent handoff
/// manifest; standalone users can omit coordination entirely.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CoordinationState {
    pub browser_ws: String,
    pub target_id: String,
    pub url: String,
    pub title: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HandoffRequest {
    pub request_id: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CoordinationSignals {
    pub owner: Option<String>,
    pub handoff: Option<HandoffRequest>,
}

/// Optional product-owned coordination boundary for live browser sessions.
/// Implementations must preserve concurrent external fields when updating
/// driver-owned state and must make `remove` bounded by the supplied timeout.
pub trait BrowserCoordination: Debug + Send + Sync + 'static {
    fn prepare(&self, panel_local_id: &str, timeout: Duration) -> bool;
    /// # Errors
    /// Returns an I/O error when initial coordination state cannot be stored.
    fn initialize(&self, panel_local_id: &str, state: &CoordinationState) -> std::io::Result<()>;
    /// # Errors
    /// Returns an I/O error when live coordination state cannot be updated.
    fn update(&self, panel_local_id: &str, state: &CoordinationState) -> std::io::Result<()>;
    /// # Errors
    /// Returns an I/O error when the activity signal cannot be stored.
    fn set_user_active(&self, panel_local_id: &str, active: bool) -> std::io::Result<()>;
    /// # Errors
    /// Returns an I/O error when host-owned signals cannot be read.
    fn signals(&self, panel_local_id: &str) -> std::io::Result<CoordinationSignals>;
    /// # Errors
    /// Returns an I/O error when the exact handoff cannot be updated.
    fn acknowledge_handoff(&self, panel_local_id: &str, request_id: &str) -> std::io::Result<bool>;
    fn remove(&self, panel_local_id: &str, timeout: Duration) -> bool;
}
