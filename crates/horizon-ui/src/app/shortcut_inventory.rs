use horizon_core::{AppShortcuts, ShortcutBinding};

use crate::terminal_widget::SSH_RECONNECT_SHORTCUT;

const GLOBAL_SHORTCUT_COUNT: usize = 19;

pub(crate) fn global_shortcut_bindings(shortcuts: &AppShortcuts) -> [ShortcutBinding; GLOBAL_SHORTCUT_COUNT] {
    [
        shortcuts.command_palette,
        shortcuts.new_terminal,
        shortcuts.focus_active_workspace,
        shortcuts.fit_active_workspace,
        shortcuts.open_remote_hosts,
        shortcuts.toggle_sessions,
        shortcuts.toggle_sidebar,
        shortcuts.toggle_hud,
        shortcuts.toggle_minimap,
        shortcuts.align_workspaces_horizontally,
        shortcuts.toggle_settings,
        shortcuts.zoom_reset,
        shortcuts.zoom_in,
        shortcuts.zoom_out,
        shortcuts.fullscreen_panel,
        shortcuts.exit_fullscreen_panel,
        shortcuts.fullscreen_window,
        shortcuts.save_editor,
        shortcuts.search,
    ]
}

pub(crate) fn ssh_reconnect_shortcut_conflicts(shortcuts: &AppShortcuts) -> bool {
    global_shortcut_bindings(shortcuts)
        .into_iter()
        .any(|binding| binding.overlaps(SSH_RECONNECT_SHORTCUT))
}

pub(crate) fn copy_selection_shortcut_label(primary_label: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("{primary_label}+C")
    } else if cfg!(target_os = "windows") {
        format!("{primary_label}+Shift+C / {primary_label}+Insert")
    } else {
        format!("{primary_label}+Shift+C")
    }
}

pub(crate) fn paste_shortcut_label(primary_label: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("{primary_label}+V")
    } else if cfg!(target_os = "windows") {
        format!("{primary_label}+Shift+V / Shift+Insert")
    } else {
        format!("{primary_label}+Shift+V")
    }
}

#[cfg(test)]
mod tests {
    use horizon_core::{ShortcutBinding, ShortcutKey, ShortcutModifiers};

    use super::{
        copy_selection_shortcut_label, global_shortcut_bindings, paste_shortcut_label, ssh_reconnect_shortcut_conflicts,
    };
    use crate::terminal_widget::SSH_RECONNECT_SHORTCUT;

    #[test]
    fn default_global_shortcuts_leave_local_ssh_reconnect_available() {
        assert!(!ssh_reconnect_shortcut_conflicts(&horizon_core::AppShortcuts::default()));
    }

    #[test]
    fn matching_global_shortcut_disables_local_ssh_reconnect() {
        let shortcuts = horizon_core::AppShortcuts {
            open_remote_hosts: SSH_RECONNECT_SHORTCUT,
            ..horizon_core::AppShortcuts::default()
        };

        assert!(ssh_reconnect_shortcut_conflicts(&shortcuts));
    }

    #[test]
    fn overlapping_mac_command_shortcut_disables_local_ssh_reconnect() {
        let shortcuts = horizon_core::AppShortcuts {
            open_remote_hosts: ShortcutBinding::new(
                ShortcutModifiers::MAC_CMD.plus(ShortcutModifiers::SHIFT),
                ShortcutKey::Letter('R'),
            ),
            ..horizon_core::AppShortcuts::default()
        };

        assert!(ssh_reconnect_shortcut_conflicts(&shortcuts));
    }

    #[test]
    fn save_editor_shortcut_conflict_disables_local_ssh_reconnect() {
        let shortcuts = horizon_core::AppShortcuts {
            save_editor: SSH_RECONNECT_SHORTCUT,
            ..horizon_core::AppShortcuts::default()
        };

        assert!(ssh_reconnect_shortcut_conflicts(&shortcuts));
    }

    #[test]
    fn global_shortcut_bindings_include_command_palette_and_search_bindings() {
        let shortcuts = horizon_core::AppShortcuts {
            command_palette: ShortcutBinding::new(ShortcutModifiers::PRIMARY_SHIFT, ShortcutKey::Letter('P')),
            search: ShortcutBinding::new(ShortcutModifiers::PRIMARY_SHIFT, ShortcutKey::Letter('G')),
            ..horizon_core::AppShortcuts::default()
        };

        let bindings = global_shortcut_bindings(&shortcuts);
        assert!(bindings.contains(&shortcuts.command_palette));
        assert!(bindings.contains(&shortcuts.search));
    }

    #[test]
    fn copy_and_paste_shortcut_labels_match_platform_conventions() {
        let primary_label = if cfg!(target_os = "macos") { "Cmd" } else { "Ctrl" };
        let copy = copy_selection_shortcut_label(primary_label);
        let paste = paste_shortcut_label(primary_label);

        if cfg!(target_os = "macos") {
            assert_eq!(copy, "Cmd+C");
            assert_eq!(paste, "Cmd+V");
        } else if cfg!(target_os = "windows") {
            assert_eq!(copy, "Ctrl+Shift+C / Ctrl+Insert");
            assert_eq!(paste, "Ctrl+Shift+V / Shift+Insert");
        } else {
            assert_eq!(copy, "Ctrl+Shift+C");
            assert_eq!(paste, "Ctrl+Shift+V");
        }
    }
}
