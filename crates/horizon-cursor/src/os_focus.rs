//! Whether this process currently owns the OS-focused window.

use std::cell::RefCell;
use std::time::{Duration, Instant};

/// Speech input polls this from the frame loop. Reuse a recent observation so
/// background desktop injection does not walk the X11 tree every egui pass.
const OS_FOCUS_CACHE_TTL: Duration = Duration::from_millis(100);

thread_local! {
    static OS_FOCUS_CACHE: RefCell<Option<(Instant, Option<bool>)>> = const { RefCell::new(None) };
}

/// `None` means this session cannot determine OS focus (typical on pure Wayland).
#[must_use]
pub fn current_process_has_os_focus() -> Option<bool> {
    observe_process_os_focus(true)
}

/// Same as [`current_process_has_os_focus`], but always re-queries X11.
///
/// Use this at transcript delivery so a stale cache cannot reroute a result.
#[must_use]
pub fn current_process_has_os_focus_fresh() -> Option<bool> {
    observe_process_os_focus(false)
}

fn observe_process_os_focus(allow_cache: bool) -> Option<bool> {
    OS_FOCUS_CACHE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let now = Instant::now();
        if allow_cache
            && let Some((cached_at, value)) = *slot
            && os_focus_cache_fresh(cached_at, now)
        {
            return value;
        }
        let value = platform::current_process_has_os_focus();
        *slot = Some((now, value));
        value
    })
}

fn os_focus_cache_fresh(cached_at: Instant, now: Instant) -> bool {
    now.saturating_duration_since(cached_at) < OS_FOCUS_CACHE_TTL
}

/// X11 input-focus window id, if this session can observe one.
#[must_use]
pub fn current_input_focus_window() -> Option<u32> {
    platform::current_input_focus_window()
}

/// `_NET_WM_PID` of `window`, walking parents when the focused child has none.
#[must_use]
pub fn window_process_id(window: u32) -> Option<u32> {
    platform::window_process_id(window)
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

    pub(super) fn current_input_focus_window() -> Option<u32> {
        DISPLAY.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                *slot = x11rb::connect(None).ok();
            }
            let (conn, screen_num) = slot.as_ref()?;
            let root = conn.setup().roots.get(*screen_num)?.root;
            let focus = conn
                .get_input_focus()
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .map(|reply| reply.focus)?;
            is_focus_candidate(focus, root).then_some(focus)
        })
    }

    pub(super) fn window_process_id(window: Window) -> Option<u32> {
        DISPLAY.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                *slot = x11rb::connect(None).ok();
            }
            let (conn, screen_num) = slot.as_ref()?;
            let root = conn.setup().roots.get(*screen_num)?.root;
            let (_, net_pid) = net_atoms(conn)?;
            pid_for_window_or_parent(conn, window, root, net_pid)
        })
    }

    fn pid_for_window_or_parent(conn: &RustConnection, window: Window, root: Window, pid_atom: Atom) -> Option<u32> {
        if !is_focus_candidate(window, root) {
            return None;
        }
        let mut current = window;
        for _ in 0..MAX_PARENT_WALKS {
            if let Some(pid) = window_pid(conn, current, pid_atom) {
                return Some(pid);
            }
            let tree = conn.query_tree(current).ok()?.reply().ok()?;
            if tree.parent == NONE || tree.parent == root || tree.parent == current {
                return window_pid(conn, tree.parent, pid_atom);
            }
            current = tree.parent;
        }
        None
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
            if let Some(window) = preferred_focus_window(input_focus, active, root) {
                collect_pids_around(conn, window, root, net_pid, &mut pids);
            }
            Some(pids)
        })
    }

    /// `KEY_STRING` is delivered to the input-focus window, so that candidate
    /// owns routing. `_NET_ACTIVE_WINDOW` is only a fallback when input focus
    /// cannot be attributed to a real client window.
    fn preferred_focus_window(input_focus: Option<Window>, active: Option<Window>, root: Window) -> Option<Window> {
        [input_focus, active]
            .into_iter()
            .find_map(|window| window.filter(|candidate| is_focus_candidate(*candidate, root)))
    }

    fn collect_pids_around(conn: &RustConnection, window: Window, root: Window, pid_atom: Atom, pids: &mut Vec<u32>) {
        let mut current = window;
        let mut windows_visited = 0;
        for _ in 0..MAX_PARENT_WALKS {
            push_pid(pids, window_pid(conn, current, pid_atom));
            let Some(tree) = conn.query_tree(current).ok().and_then(|cookie| cookie.reply().ok()) else {
                break;
            };
            if current == window {
                collect_child_pids(
                    conn,
                    &tree.children,
                    pid_atom,
                    CHILD_WALK_DEPTH,
                    pids,
                    &mut windows_visited,
                );
            }
            if tree.parent == NONE || tree.parent == root || tree.parent == current {
                push_pid(pids, window_pid(conn, tree.parent, pid_atom));
                break;
            }
            current = tree.parent;
        }
    }

    fn collect_child_pids(
        conn: &RustConnection,
        children: &[Window],
        pid_atom: Atom,
        depth: u8,
        pids: &mut Vec<u32>,
        windows_visited: &mut usize,
    ) {
        if depth == 0 || child_window_budget_exhausted(*windows_visited) {
            return;
        }
        for &child in children {
            if child_window_budget_exhausted(*windows_visited) {
                return;
            }
            if !is_real_window(child) {
                continue;
            }
            *windows_visited += 1;
            push_pid(pids, window_pid(conn, child, pid_atom));
            if depth == 1 {
                continue;
            }
            let Some(tree) = conn.query_tree(child).ok().and_then(|cookie| cookie.reply().ok()) else {
                continue;
            };
            collect_child_pids(
                conn,
                &tree.children,
                pid_atom,
                depth.saturating_sub(1),
                pids,
                windows_visited,
            );
        }
    }

    const fn child_window_budget_exhausted(windows_visited: usize) -> bool {
        windows_visited >= MAX_CHILD_WINDOWS
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

    const fn is_focus_candidate(window: Window, root: Window) -> bool {
        is_real_window(window) && window != root
    }

    #[cfg(test)]
    mod tests {
        use super::{
            MAX_CHILD_WINDOWS, NONE, POINTER_ROOT, child_window_budget_exhausted, is_focus_candidate, is_real_window,
            preferred_focus_window, process_owns_focus,
        };

        #[test]
        fn x11_none_and_pointer_root_are_not_client_windows() {
            assert!(!is_real_window(NONE));
            assert!(!is_real_window(POINTER_ROOT));
            assert!(is_real_window(0x3c0_0004));
        }

        #[test]
        fn screen_root_is_not_a_focus_candidate() {
            const ROOT: u32 = 0x21;
            assert!(!is_focus_candidate(ROOT, ROOT));
            assert!(!is_focus_candidate(NONE, ROOT));
            assert!(is_focus_candidate(0x3c0_0004, ROOT));
        }

        #[test]
        fn input_focus_window_wins_over_stale_active_window() {
            const ROOT: u32 = 0x21;
            const HORIZON: u32 = 0x3c0_0004;
            const TEAMS: u32 = 0x3c0_0005;
            assert_eq!(preferred_focus_window(Some(TEAMS), Some(HORIZON), ROOT), Some(TEAMS));
            assert_eq!(preferred_focus_window(None, Some(HORIZON), ROOT), Some(HORIZON));
            assert_eq!(preferred_focus_window(Some(ROOT), Some(HORIZON), ROOT), Some(HORIZON));
            assert_eq!(preferred_focus_window(Some(NONE), Some(HORIZON), ROOT), Some(HORIZON));
            assert_eq!(preferred_focus_window(None, None, ROOT), None);
        }

        #[test]
        fn compositor_frame_pid_does_not_hide_the_client_process() {
            assert_eq!(process_owns_focus(16617, &[6766, 16617]), Some(true));
            assert_eq!(process_owns_focus(16617, &[14474]), Some(false));
            assert_eq!(process_owns_focus(16617, &[]), None);
        }

        #[test]
        fn child_walk_budget_counts_windows_not_pids() {
            assert_eq!(MAX_CHILD_WINDOWS, 64);
            assert!(!child_window_budget_exhausted(0));
            assert!(!child_window_budget_exhausted(MAX_CHILD_WINDOWS - 1));
            assert!(child_window_budget_exhausted(MAX_CHILD_WINDOWS));
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
mod platform {
    pub(super) const fn current_process_has_os_focus() -> Option<bool> {
        None
    }

    pub(super) const fn current_input_focus_window() -> Option<u32> {
        None
    }

    pub(super) const fn window_process_id(_window: u32) -> Option<u32> {
        None
    }
}

#[cfg(test)]
mod cache_tests {
    use std::time::{Duration, Instant};

    use super::{OS_FOCUS_CACHE_TTL, os_focus_cache_fresh};

    #[test]
    fn os_focus_cache_covers_a_speech_poll_interval() {
        let start = Instant::now();
        assert_eq!(OS_FOCUS_CACHE_TTL, Duration::from_millis(100));
        assert!(os_focus_cache_fresh(start, start + Duration::from_millis(99)));
        assert!(!os_focus_cache_fresh(start, start + Duration::from_millis(100)));
    }
}
