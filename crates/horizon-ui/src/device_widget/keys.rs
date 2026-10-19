//! egui's keys, buttons, modifiers and positions as `horizon_vnc` input. The
//! input rules themselves, such as what is sent and when it is released, are
//! in `horizon_vnc::InputState`.
use egui::{Key, PointerButton, Pos2, Rect};
use horizon_vnc::input::{
    XK_BACKSPACE, XK_DELETE, XK_DOWN, XK_END, XK_ESCAPE, XK_F1, XK_HOME, XK_INSERT, XK_LEFT, XK_PAGE_DOWN, XK_PAGE_UP,
    XK_RETURN, XK_RIGHT, XK_TAB, XK_UP,
};
use horizon_vnc::{
    BUTTON_LEFT, BUTTON_MIDDLE, BUTTON_RIGHT, DesktopPoint, ImageLayout, InputEvent, InputState, ScreenRect,
    keysym_for_char,
};

/// A key press or release. A key without a keysym sends nothing.
pub(super) fn key(input: &mut InputState, pressed: Key, down: bool, modifiers: egui::Modifiers) -> Vec<InputEvent> {
    keysym_for_key(pressed).map_or_else(Vec::new, |keysym| input.key(keysym, down, self::modifiers(modifiers)))
}

/// The modifier keys as the desktop sees them; egui reports Command as `mac_cmd`.
pub(super) const fn modifiers(modifiers: egui::Modifiers) -> horizon_vnc::Modifiers {
    horizon_vnc::Modifiers {
        shift: modifiers.shift,
        ctrl: modifiers.ctrl,
        alt: modifiers.alt,
        command: modifiers.mac_cmd,
    }
}

/// Map a point inside the rendered image back to the desktop pixel it shows,
/// honouring the active crop and scaling. Outside the image is `None`.
pub(super) fn desktop_point(image: Rect, layout: &ImageLayout, pos: Pos2) -> Option<DesktopPoint> {
    let image = ScreenRect {
        min: [image.min.x, image.min.y],
        max: [image.max.x, image.max.y],
    };
    horizon_vnc::desktop_point(image, layout, [pos.x, pos.y])
}

/// The RFB mask bit for one of egui's pointer buttons; extra buttons have none.
pub(super) const fn button_bit(button: PointerButton) -> Option<u8> {
    match button {
        PointerButton::Primary => Some(BUTTON_LEFT),
        PointerButton::Secondary => Some(BUTTON_RIGHT),
        PointerButton::Middle => Some(BUTTON_MIDDLE),
        PointerButton::Extra1 | PointerButton::Extra2 => None,
    }
}

/// The keysym for a key egui names; modifier keys are handled by state diffing
/// and keys without a keysym (browser and clipboard keys) are dropped.
pub(super) fn keysym_for_key(pressed: Key) -> Option<u32> {
    let keysym = match pressed {
        Key::ArrowDown => XK_DOWN,
        Key::ArrowLeft => XK_LEFT,
        Key::ArrowRight => XK_RIGHT,
        Key::ArrowUp => XK_UP,
        Key::Escape => XK_ESCAPE,
        Key::Tab => XK_TAB,
        Key::Backspace => XK_BACKSPACE,
        Key::Enter => XK_RETURN,
        Key::Insert => XK_INSERT,
        Key::Delete => XK_DELETE,
        Key::Home => XK_HOME,
        Key::End => XK_END,
        Key::PageUp => XK_PAGE_UP,
        Key::PageDown => XK_PAGE_DOWN,
        Key::Space => u32::from(' '),
        Key::Minus => u32::from('-'),
        Key::Quote => u32::from('\''),
        Key::Copy
        | Key::Cut
        | Key::Paste
        | Key::BrowserBack
        | Key::ShiftLeft
        | Key::ShiftRight
        | Key::ControlLeft
        | Key::ControlRight
        | Key::AltLeft
        | Key::AltRight
        | Key::SuperLeft
        | Key::SuperRight
        | Key::IntlBackslash => return None,
        other => {
            let name = other.symbol_or_name();
            if let Some(number) = name.strip_prefix('F').and_then(|digits| digits.parse::<u32>().ok())
                && (1..=35).contains(&number)
                && name.len() <= 3
            {
                XK_F1 + number - 1
            } else {
                let mut characters = name.chars();
                match (characters.next(), characters.next()) {
                    (Some(character), None) if character.is_ascii() => keysym_for_char(character.to_ascii_lowercase()),
                    _ => return None,
                }
            }
        }
    };
    Some(keysym)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_and_clipboard_keys_send_nothing_and_extra_buttons_have_no_bit() {
        let mut state = InputState::default();
        assert!(
            key(&mut state, Key::ShiftLeft, true, egui::Modifiers::SHIFT).is_empty(),
            "modifiers come from the diff"
        );
        assert!(key(&mut state, Key::Copy, true, egui::Modifiers::NONE).is_empty());
        assert!(
            key(&mut state, Key::A, true, egui::Modifiers::NONE).is_empty(),
            "text sends a"
        );
        assert_eq!(
            key(&mut state, Key::A, true, egui::Modifiers::MAC_CMD),
            [InputEvent::Key {
                keysym: u32::from('a'),
                down: true
            }],
            "Command is the Super modifier, so the letter is a chord"
        );
        assert_eq!(button_bit(PointerButton::Middle), Some(2));
        assert_eq!(button_bit(PointerButton::Extra1), None);
    }

    #[test]
    fn egui_positions_map_to_desktop_pixels_with_inclusive_edges() {
        let image = Rect::from_min_size(Pos2::new(100.0, 50.0), egui::vec2(200.0, 100.0));
        let layout = ImageLayout {
            source: horizon_vnc::Region::whole([1600, 800]),
            output: [200, 100],
        };
        assert_eq!(
            desktop_point(image, &layout, image.max),
            Some(DesktopPoint { x: 1599, y: 799 })
        );
        assert_eq!(
            desktop_point(image, &layout, Pos2::new(200.0, 50.0)),
            Some(DesktopPoint { x: 800, y: 0 })
        );
        assert_eq!(desktop_point(image, &layout, Pos2::new(301.0, 50.0)), None);
    }

    #[test]
    fn key_names_resolve_to_the_expected_keysyms() {
        assert_eq!(keysym_for_key(Key::F1), Some(XK_F1));
        assert_eq!(keysym_for_key(Key::F12), Some(XK_F1 + 11));
        assert_eq!(keysym_for_key(Key::F35), Some(XK_F1 + 34));
        assert_eq!(keysym_for_key(Key::Num7), Some(u32::from('7')));
        assert_eq!(keysym_for_key(Key::Z), Some(u32::from('z')));
        assert_eq!(keysym_for_key(Key::Colon), Some(u32::from(':')));
        assert_eq!(
            keysym_for_key(Key::Minus),
            Some(u32::from('-')),
            "not the typographic minus"
        );
        assert_eq!(keysym_for_key(Key::Quote), Some(u32::from('\'')));
        assert_eq!(keysym_for_key(Key::Space), Some(u32::from(' ')));
        assert_eq!(keysym_for_key(Key::Backtick), Some(u32::from('`')));
        assert_eq!(keysym_for_key(Key::Delete), Some(XK_DELETE));
        assert_eq!(keysym_for_key(Key::BrowserBack), None);
        assert_eq!(keysym_for_key(Key::IntlBackslash), None);
    }
}
