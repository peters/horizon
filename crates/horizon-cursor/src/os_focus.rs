//! Whether this process currently owns the OS-focused window.

/// `None` means this session cannot determine OS focus (typical on pure Wayland).
#[must_use]
pub fn current_process_has_os_focus() -> Option<bool> {
    platform::current_process_has_os_focus()
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod platform {
    use std::cell::RefCell;
    use std::sync::OnceLock;

    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt as _, Window};
    use x11rb::rust_connection::RustConnection;

    const NONE: Window = 0;
    const POINTER_ROOT: Window = 1;
    const MAX_PARENT_WALKS: usize = 32;
    static NET_ATOMS: OnceLock<(Atom, Atom)> = OnceLock::new();

    thread_local! {
        static DISPLAY: RefCell<Option<(RustConnection, usize)>> = const { RefCell::new(None) };
    }

    pub(super) fn current_process_has_os_focus() -> Option<bool> {
        let focused_pid = focused_window_pid()?;
        Some(focused_pid == std::process::id())
    }

    fn focused_window_pid() -> Option<u32> {
        DISPLAY.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                *slot = x11rb::connect(None).ok();
            }
            let (conn, screen_num) = slot.as_ref()?;
            let root = conn.setup().roots.get(*screen_num)?.root;
            let (net_active, net_pid) = net_atoms(conn)?;
            let active = window_from_property(conn, root, net_active, AtomEnum::WINDOW);
            let input_focus = conn
                .get_input_focus()
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .map(|reply| reply.focus);
            for window in [active, input_focus]
                .into_iter()
                .flatten()
                .filter(|window| is_real_window(*window))
            {
                if let Some(pid) = pid_from_window_or_parents(conn, window, root, net_pid) {
                    return Some(pid);
                }
            }
            None
        })
    }

    fn net_atoms(conn: &RustConnection) -> Option<(Atom, Atom)> {
        if let Some(atoms) = NET_ATOMS.get() {
            return Some(*atoms);
        }
        let atoms = (
            intern_atom(conn, b"_NET_ACTIVE_WINDOW")?,
            intern_atom(conn, b"_NET_WM_PID")?,
        );
        Some(*NET_ATOMS.get_or_init(|| atoms))
    }

    fn intern_atom<C: x11rb::connection::Connection>(conn: &C, name: &[u8]) -> Option<Atom> {
        conn.intern_atom(false, name).ok()?.reply().ok().map(|reply| reply.atom)
    }

    fn window_from_property<C: x11rb::connection::Connection>(
        conn: &C,
        window: Window,
        property: Atom,
        type_: impl Into<Atom>,
    ) -> Option<Window> {
        let reply = conn
            .get_property(false, window, property, type_, 0, 1)
            .ok()?
            .reply()
            .ok()?;
        (reply.format == 32).then_some(())?;
        reply.value32()?.next()
    }

    fn pid_from_window_or_parents<C: x11rb::connection::Connection>(
        conn: &C,
        mut window: Window,
        root: Window,
        pid_atom: Atom,
    ) -> Option<u32> {
        for _ in 0..MAX_PARENT_WALKS {
            if let Some(pid) = window_pid(conn, window, pid_atom) {
                return Some(pid);
            }
            let tree = conn.query_tree(window).ok()?.reply().ok()?;
            if tree.parent == NONE || tree.parent == root || tree.parent == window {
                return window_pid(conn, tree.parent, pid_atom);
            }
            window = tree.parent;
        }
        None
    }

    fn window_pid<C: x11rb::connection::Connection>(conn: &C, window: Window, pid_atom: Atom) -> Option<u32> {
        if !is_real_window(window) {
            return None;
        }
        let reply = conn
            .get_property(false, window, pid_atom, AtomEnum::CARDINAL, 0, 1)
            .ok()?
            .reply()
            .ok()?;
        (reply.format == 32).then_some(())?;
        reply.value32()?.next()
    }

    const fn is_real_window(window: Window) -> bool {
        window != NONE && window != POINTER_ROOT
    }

    #[cfg(test)]
    mod tests {
        use super::{NONE, POINTER_ROOT, is_real_window};

        #[test]
        fn x11_none_and_pointer_root_are_not_client_windows() {
            assert!(!is_real_window(NONE));
            assert!(!is_real_window(POINTER_ROOT));
            assert!(is_real_window(0x3c0_0004));
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
mod platform {
    pub(super) const fn current_process_has_os_focus() -> Option<bool> {
        None
    }
}
