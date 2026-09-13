use crate::{BrowserInput, BrowserVideoCaptureOptions, BrowserVideoOperation};

/// What a host asks a live browser driver to do.
#[derive(Clone, Debug)]
pub enum BrowserCommand {
    Navigate(String),
    Reload,
    Back,
    Forward,
    SetViewport {
        width: u32,
        height: u32,
    },
    Input(BrowserInput),
    /// Start, pause, resume, inspect, or stop page-pixel `WebM` capture.
    Video {
        operation: BrowserVideoOperation,
        options: Option<BrowserVideoCaptureOptions>,
    },
    /// The user finished steering and handed control back to the agent.
    HandoffDone,
    Stop,
}
