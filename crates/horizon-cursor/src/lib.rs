//! Lightweight platform cursor position query.
//!
//! Returns global (screen-space) coordinates without requiring special
//! permissions.  On macOS this deliberately avoids the Accessibility
//! subsystem — only Core Graphics is used.

/// Query the current global cursor position in screen (root-window)
/// coordinates.  Returns `None` when the platform backend is
/// unavailable (e.g. no X11 display on a Wayland-only session).
#[must_use]
pub fn cursor_position() -> Option<(i32, i32)> {
    platform::cursor_position()
}

// ---- platform backends ------------------------------------------------

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod platform {
    pub(crate) fn cursor_position() -> Option<(i32, i32)> {
        use x11rb::connection::Connection as _;
        use x11rb::protocol::xproto::ConnectionExt as _;

        let (conn, screen_num) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots.get(screen_num)?.root;
        let reply = conn.query_pointer(root).ok()?.reply().ok()?;
        Some((i32::from(reply.root_x), i32::from(reply.root_y)))
    }
}

#[cfg(target_os = "windows")]
mod platform {
    pub(crate) fn cursor_position() -> Option<(i32, i32)> {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

        let mut point = POINT { x: 0, y: 0 };
        // SAFETY: `GetCursorPos` writes into the provided POINT and returns
        // a BOOL.  The pointer is valid for the lifetime of the local.
        let ok = unsafe { GetCursorPos(&mut point) };
        (ok != 0).then_some((point.x, point.y))
    }
}

#[cfg(target_os = "macos")]
mod platform {
    fn screen_coordinate(value: f64) -> Option<i32> {
        if !value.is_finite() {
            return None;
        }

        let rounded = value.round().clamp(f64::from(i32::MIN), f64::from(i32::MAX));
        #[allow(clippy::cast_possible_truncation)]
        {
            Some(rounded as i32)
        }
    }

    pub(crate) fn cursor_position() -> Option<(i32, i32)> {
        use core_graphics::event::CGEvent;
        use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok()?;
        let event = CGEvent::new(source).ok()?;
        let point = event.location();
        Some((screen_coordinate(point.x)?, screen_coordinate(point.y)?))
    }

    #[cfg(test)]
    mod tests {
        use super::screen_coordinate;

        #[test]
        fn screen_coordinate_rounds_and_clamps_finite_values() {
            assert_eq!(screen_coordinate(12.6), Some(13));
            assert_eq!(screen_coordinate(-12.4), Some(-12));
            assert_eq!(screen_coordinate(f64::INFINITY), None);
            assert_eq!(screen_coordinate(f64::from(i32::MAX) + 512.0), Some(i32::MAX));
            assert_eq!(screen_coordinate(f64::from(i32::MIN) - 512.0), Some(i32::MIN));
        }
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "windows",
    target_os = "macos"
)))]
mod platform {
    pub(crate) fn cursor_position() -> Option<(i32, i32)> {
        None
    }
}
