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

impl BrowserCommand {
    /// Whether a user-originated command means the user is actively steering
    /// the page and should temporarily pause external agent actions.
    #[must_use]
    pub fn is_user_activity(&self) -> bool {
        match self {
            Self::Navigate(_) | Self::Reload | Self::Back | Self::Forward => true,
            Self::Input(input) => input.is_activity(),
            Self::SetViewport { .. } | Self::HandoffDone | Self::Stop => false,
        }
    }
}
