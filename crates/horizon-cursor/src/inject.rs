//! Desktop text-injection errors and shared X11 key mapping.

use std::fmt;

/// Failure to inject input into the focused desktop client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InjectError {
    /// This session does not provide the requested injection mechanism.
    Unsupported,
    /// The focused target cannot safely accept the requested input.
    Target(&'static str),
    /// The platform backend failed while injecting input.
    Failed(&'static str),
}

impl fmt::Display for InjectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => formatter.write_str("desktop text injection is unavailable in this session"),
            Self::Target(message) | Self::Failed(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for InjectError {}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub(crate) use platform::keysym_to_keycode_with_shift;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod platform {
    use x11rb::protocol::xproto::ConnectionExt as _;

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

#[cfg(test)]
mod tests {
    use super::InjectError;

    #[test]
    fn unsupported_display_is_distinct_from_backend_failure() {
        assert_ne!(
            InjectError::Unsupported,
            InjectError::Failed("accessibility bus failed")
        );
        assert_ne!(
            InjectError::Target("focused field is not editable"),
            InjectError::Failed("focused field is not editable")
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
}
