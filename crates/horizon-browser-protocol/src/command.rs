use crate::{BrowserInput, BrowserVideoCaptureOverrides, BrowserVideoOperation};

/// What a host asks a live browser driver to do.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum BrowserCommand {
    Navigate(String),
    Reload,
    Back,
    Forward,
    SetViewport {
        width: u32,
        height: u32,
    },
    /// Rotate a remote device and await measured acknowledgement.
    Orientation {
        action_id: String,
        orientation: crate::remote::RemoteOrientation,
    },
    Input(BrowserInput),
    /// Deliver host files at a point in the page viewport.
    DropFiles {
        x: f64,
        y: f64,
        paths: Vec<std::path::PathBuf>,
    },
    /// Start, pause, resume, inspect, or stop page-pixel `WebM` capture.
    Video {
        operation: BrowserVideoOperation,
        options: Option<BrowserVideoCaptureOverrides>,
    },
    /// The user finished steering and handed control back to the agent.
    HandoffDone,
    /// Choose an option from the host-owned native `<select>` overlay.
    NativeSelectChoose {
        index: u32,
    },
    /// Dismiss the host-owned native `<select>` overlay without changing value.
    NativeSelectDismiss,
    Stop,
}
