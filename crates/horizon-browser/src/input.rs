//! Browser input values and engine-owned input policy/adapters.

mod cdp;

pub(crate) use cdp::BrowserInputCdpExt;
pub use horizon_browser_protocol::input::*;

use horizon_browser_protocol::BrowserCommand;

/// Whether a user-originated command should temporarily pause agent actions.
pub(crate) fn is_user_activity(command: &BrowserCommand) -> bool {
    match command {
        BrowserCommand::Navigate(_) | BrowserCommand::Reload | BrowserCommand::Back | BrowserCommand::Forward => true,
        BrowserCommand::Input(input) => is_activity(input),
        BrowserCommand::SetViewport { .. } | BrowserCommand::HandoffDone | BrowserCommand::Stop => false,
    }
}

pub(crate) fn is_activity(input: &BrowserInput) -> bool {
    matches!(
        input,
        BrowserInput::MousePress { .. }
            | BrowserInput::Wheel { .. }
            | BrowserInput::KeyDown { .. }
            | BrowserInput::InsertText { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_activity_distinguishes_motion_and_state_changes() {
        let modifiers = BrowserModifiers::none();
        assert!(!is_activity(&BrowserInput::MouseMove {
            x: 1.0,
            y: 2.0,
            buttons: 0,
            modifiers,
        }));
        assert!(is_activity(&BrowserInput::MousePress {
            x: 1.0,
            y: 2.0,
            button: BrowserButton::Left,
            click_count: 1,
            buttons: 1,
            modifiers,
        }));
        assert!(is_activity(&BrowserInput::InsertText {
            text: "test".to_string(),
        }));
    }
}
