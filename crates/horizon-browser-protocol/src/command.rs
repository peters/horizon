use crate::BrowserInput;

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
    /// The user finished steering and handed control back to the agent.
    HandoffDone,
    Stop,
}
