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
    const CHILD_WALK_DEPTH: u8 = 2;
    const MAX_CHILD_WINDOWS: usize = 64;
    static NET_ATOMS: OnceLock<(Atom, Atom)> = OnceLock::new();

    thread_local! {
        static DISPLAY: RefCell<Option<(RustConnection, usize)>> = const { RefCell::new(None) };
    }

    pub(super) fn current_process_has_os_focus() -> Option<bool> {
        process_owns_focus(std::process::id(), &focused_candidate_pids()?)
    }

    fn focused_candidate_pids() -> Option<Vec<u32>> {
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
            let mut pids = Vec::new();
            for window in [active, input_focus]
                .into_iter()
                .flatten()
                .filter(|window| is_real_window(*window))
            {
                collect_pids_around(conn, window, root, net_pid, &mut pids);
            }
            Some(pids)
        })
    }

    fn collect_pids_around(conn: &RustConnection, window: Window, root: Window, pid_atom: Atom, pids: &mut Vec<u32>) {
        let mut current = window;
        for _ in 0..MAX_PARENT_WALKS {
            push_pid(pids, window_pid(conn, current, pid_atom));
            let Some(tree) = conn.query_tree(current).ok().and_then(|cookie| cookie.reply().ok()) else {
                break;
            };
            if current == window {
                collect_child_pids(conn, &tree.children, pid_atom, CHILD_WALK_DEPTH, pids);
            }
            if tree.parent == NONE || tree.parent == root || tree.parent == current {
                push_pid(pids, window_pid(conn, tree.parent, pid_atom));
                break;
            }
            current = tree.parent;
        }
    }

    fn collect_child_pids(conn: &RustConnection, children: &[Window], pid_atom: Atom, depth: u8, pids: &mut Vec<u32>) {
        if depth == 0 || pids.len() >= MAX_CHILD_WINDOWS {
            return;
        }
        for &child in children {
            if pids.len() >= MAX_CHILD_WINDOWS || !is_real_window(child) {
                continue;
            }
            push_pid(pids, window_pid(conn, child, pid_atom));
            if depth == 1 {
                continue;
            }
            let Some(tree) = conn.query_tree(child).ok().and_then(|cookie| cookie.reply().ok()) else {
                continue;
            };
            collect_child_pids(conn, &tree.children, pid_atom, depth.saturating_sub(1), pids);
        }
    }

    fn push_pid(pids: &mut Vec<u32>, pid: Option<u32>) {
        if let Some(pid) = pid
            && !pids.contains(&pid)
        {
            pids.push(pid);
        }
    }

    fn process_owns_focus(our_pid: u32, candidate_pids: &[u32]) -> Option<bool> {
        if candidate_pids.contains(&our_pid) {
            Some(true)
        } else if candidate_pids.is_empty() {
            None
        } else {
            Some(false)
        }
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
        use super::{NONE, POINTER_ROOT, is_real_window, process_owns_focus};

        #[test]
        fn x11_none_and_pointer_root_are_not_client_windows() {
            assert!(!is_real_window(NONE));
            assert!(!is_real_window(POINTER_ROOT));
            assert!(is_real_window(0x3c0_0004));
        }

        #[test]
        fn compositor_frame_pid_does_not_hide_the_client_process() {
            assert_eq!(process_owns_focus(16617, &[6766, 16617]), Some(true));
            assert_eq!(process_owns_focus(16617, &[14474]), Some(false));
            assert_eq!(process_owns_focus(16617, &[]), None);
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
mod platform {
    pub(super) const fn current_process_has_os_focus() -> Option<bool> {
        None
    }
}
