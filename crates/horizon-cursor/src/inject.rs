//! Paste-chord injection into the currently focused OS client.

use std::fmt;

/// Failure to inject a paste chord into the focused client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InjectError {
    /// This session cannot synthesize a paste chord (typical on pure Wayland).
    Unsupported,
    /// The platform backend failed while sending the chord.
    Failed(&'static str),
    /// The transcript never reached the clipboard, so there is nothing to paste.
    Clipboard(&'static str),
}

impl fmt::Display for InjectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => formatter.write_str("paste chord is not available on this display"),
            Self::Failed(message) | Self::Clipboard(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for InjectError {}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub(crate) use platform::keysym_to_keycode_with_shift;

/// Send Ctrl+V to the focused client without changing focus.
///
/// Currently implemented on X11 via `XTest`. macOS and native Wayland sessions
/// return [`InjectError::Unsupported`].
///
/// # Errors
///
/// Returns [`InjectError::Unsupported`] when this session has no X11 display,
/// or [`InjectError::Failed`] when the paste chord could not be synthesized.
pub fn send_paste_chord() -> Result<(), InjectError> {
    platform::send_paste_chord()
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod platform {
    use super::InjectError;
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{ConnectionExt as _, KEY_PRESS_EVENT, KEY_RELEASE_EVENT};
    use x11rb::protocol::xtest::ConnectionExt as _;

    const XK_CONTROL_L: u32 = 0xffe3;
    const XK_V: u32 = 0x0076;

    pub(super) fn send_paste_chord() -> Result<(), InjectError> {
        let (conn, _screen_num) = x11rb::connect(None).map_err(|_| InjectError::Unsupported)?;
        conn.xtest_get_version(2, 1)
            .map_err(|_| InjectError::Failed("XTest version query failed"))?
            .reply()
            .map_err(|_| InjectError::Failed("XTest not available"))?;
        let control = keysym_to_keycode(&conn, XK_CONTROL_L).ok_or(InjectError::Failed("Control keycode missing"))?;
        let vee = keysym_to_keycode(&conn, XK_V).ok_or(InjectError::Failed("V keycode missing"))?;
        fake_key(&conn, KEY_PRESS_EVENT, control)?;
        let press_vee = fake_key(&conn, KEY_PRESS_EVENT, vee);
        let release_vee = fake_key(&conn, KEY_RELEASE_EVENT, vee);
        let release_control = fake_key(&conn, KEY_RELEASE_EVENT, control);
        let flushed = conn.flush().map_err(|_| InjectError::Failed("X11 flush failed"));
        press_vee.and(release_vee).and(release_control).and(flushed)
    }

    fn fake_key<C: x11rb::connection::Connection>(conn: &C, event_type: u8, keycode: u8) -> Result<(), InjectError> {
        conn.xtest_fake_input(event_type, keycode, 0, 0, 0, 0, 0)
            .map_err(|_| InjectError::Failed("XTest fake_input failed"))?
            .check()
            .map_err(|_| InjectError::Failed("XTest fake_input rejected"))?;
        Ok(())
    }

    pub(crate) fn keysym_to_keycode<C: x11rb::connection::Connection>(conn: &C, keysym: u32) -> Option<u8> {
        keysym_to_keycode_with_shift(conn, keysym).map(|(keycode, _)| keycode)
    }

    pub(crate) fn keysym_to_keycode_with_shift<C: x11rb::connection::Connection>(
        conn: &C,
        keysym: u32,
    ) -> Option<(u8, bool)> {
        let setup = conn.setup();
        let min = setup.min_keycode;
        let count = setup.max_keycode.checked_sub(min)?.saturating_add(1);
        let mapping = conn.get_keyboard_mapping(min, count).ok()?.reply().ok()?;
        let per = usize::from(mapping.keysyms_per_keycode);
        if per == 0 {
            return None;
        }
        mapping.keysyms.chunks(per).enumerate().find_map(|(index, keysyms)| {
            let shift = super::shift_required_for_keysym(keysyms, keysym)?;
            let offset = u8::try_from(index).ok()?;
            Some((min.saturating_add(offset), shift))
        })
    }
}

/// X11 keysym columns alternate unshifted/shifted. Column 1 and 3 need Shift.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub(crate) fn shift_required_for_keysym(keysyms: &[u32], keysym: u32) -> Option<bool> {
    keysyms
        .iter()
        .position(|&candidate| candidate == keysym)
        .map(|column| column % 2 == 1)
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
mod platform {
    use super::InjectError;

    pub(super) fn send_paste_chord() -> Result<(), InjectError> {
        Err(InjectError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::InjectError;

    #[test]
    fn unsupported_display_is_distinct_from_backend_failure() {
        assert_ne!(InjectError::Unsupported, InjectError::Failed("XTest fake_input failed"));
        assert_ne!(
            InjectError::Clipboard("clipboard unavailable"),
            InjectError::Failed("clipboard unavailable")
        );
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[test]
    fn plus_on_the_shift_level_requires_shift() {
        const XK_EQUAL: u32 = 0x003d;
        const XK_PLUS: u32 = 0x002b;
        assert_eq!(
            super::shift_required_for_keysym(&[XK_EQUAL, XK_PLUS], XK_PLUS),
            Some(true)
        );
        assert_eq!(super::shift_required_for_keysym(&[XK_PLUS], XK_PLUS), Some(false));
        assert_eq!(super::shift_required_for_keysym(&[XK_EQUAL], XK_PLUS), None);
    }

    #[test]
    #[ignore = "live X11 nested-display smoke"]
    fn live_send_paste_chord() {
        super::send_paste_chord().expect("paste chord");
    }
}
