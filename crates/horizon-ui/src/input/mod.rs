mod keyboard;
mod mouse;
mod sequence;

pub use keyboard::{paste_bytes, translate_key_event};
pub use mouse::{WheelAction, mouse_button_report, mouse_motion_report, wheel_action};

#[derive(Clone, Copy)]
pub struct GridPoint {
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Copy, Default)]
pub struct PointerButtons {
    pub primary: bool,
    pub middle: bool,
    pub secondary: bool,
}

#[cfg(test)]
mod tests {
    use super::{
        GridPoint, PointerButtons, WheelAction, mouse_button_report, mouse_motion_report, paste_bytes,
        translate_key_event, wheel_action,
    };
    use alacritty_terminal::term::TermMode;
    use egui::{Key, Modifiers, MouseWheelUnit, PointerButton, Vec2};

    #[test]
    fn app_cursor_mode_uses_ss3_sequences() {
        let translation =
            translate_key_event(Key::ArrowUp, true, false, Modifiers::NONE, TermMode::APP_CURSOR).expect("up");

        assert_eq!(translation.bytes, b"\x1bOA");
    }

    #[test]
    fn ctrl_letter_maps_to_control_code() {
        let translation = translate_key_event(Key::C, true, false, Modifiers::CTRL, TermMode::NONE).expect("ctrl-c");

        assert_eq!(translation.bytes, vec![3]);
    }

    #[test]
    fn kitty_escape_uses_csi_u_sequence() {
        let translation = translate_key_event(
            Key::Escape,
            true,
            false,
            Modifiers::NONE,
            TermMode::DISAMBIGUATE_ESC_CODES,
        )
        .expect("kitty escape");

        assert_eq!(translation.bytes, b"\x1b[27u");
    }

    #[test]
    fn bracketed_paste_filters_escape_and_ctrl_c() {
        let bytes = paste_bytes("hi\x1bthere\x03", TermMode::BRACKETED_PASTE, true);

        assert_eq!(bytes, b"\x1b[200~hithere\x1b[201~");
    }

    #[test]
    fn sgr_mouse_reports_button_release() {
        let bytes = mouse_button_report(
            PointerButton::Primary,
            false,
            Modifiers::NONE,
            TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE,
            GridPoint { line: 3, column: 8 },
        )
        .expect("mouse release");

        assert_eq!(bytes, b"\x1b[<0;9;4m");
    }

    #[test]
    fn wheel_uses_mouse_reporting_when_enabled() {
        let action = wheel_action(
            Vec2::new(0.0, 12.0),
            MouseWheelUnit::Point,
            Vec2::new(8.0, 12.0),
            Modifiers::NONE,
            TermMode::MOUSE_REPORT_CLICK,
            GridPoint { line: 1, column: 1 },
        )
        .expect("wheel action");

        match action {
            WheelAction::Pty(bytes) => assert_eq!(bytes, b"\x1b[M`\"\""),
            WheelAction::Scrollback(_) => panic!("expected PTY wheel reporting"),
        }
    }

    #[test]
    fn wheel_falls_back_to_scrollback_without_mouse_mode() {
        let action = wheel_action(
            Vec2::new(0.0, 32.0),
            MouseWheelUnit::Point,
            Vec2::new(8.0, 16.0),
            Modifiers::NONE,
            TermMode::NONE,
            GridPoint { line: 0, column: 0 },
        )
        .expect("scrollback");

        match action {
            WheelAction::Scrollback(lines) => assert_eq!(lines, 2),
            WheelAction::Pty(_) => panic!("expected scrollback"),
        }
    }

    #[test]
    fn mouse_motion_uses_drag_report_codes() {
        let bytes = mouse_motion_report(
            PointerButtons {
                primary: true,
                middle: false,
                secondary: false,
            },
            Modifiers::NONE,
            TermMode::MOUSE_DRAG,
            GridPoint { line: 0, column: 0 },
        )
        .expect("drag motion");

        assert_eq!(bytes, b"\x1b[M@!!");
    }
}
