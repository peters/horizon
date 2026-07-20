use egui::Event;
use horizon_core::AppShortcuts;

use crate::app::shortcuts::shortcut_pressed_in_events;

// Named fields rather than a tuple: the two actions are both `bool`, so a
// swapped pair would still compile and silently rebind the detached toolbar's
// advertised shortcuts to each other's action.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct DetachedShortcutActions {
    pub fit_workspace: bool,
    pub toggle_minimap: bool,
}

pub(super) fn detached_shortcut_actions(events: &[Event], shortcuts: &AppShortcuts) -> DetachedShortcutActions {
    DetachedShortcutActions {
        fit_workspace: shortcut_pressed_in_events(events, shortcuts.fit_active_workspace),
        toggle_minimap: shortcut_pressed_in_events(events, shortcuts.toggle_minimap),
    }
}

#[cfg(test)]
mod tests {
    use egui::{Event, Key, Modifiers};
    use horizon_core::AppShortcuts;

    use super::{DetachedShortcutActions, detached_shortcut_actions};

    fn primary_shift_press(key: Key) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND | Modifiers::SHIFT,
        }
    }

    #[test]
    fn fit_shortcut_requests_only_the_fit_action() {
        let actions = detached_shortcut_actions(&[primary_shift_press(Key::Num9)], &AppShortcuts::default());

        assert_eq!(
            actions,
            DetachedShortcutActions {
                fit_workspace: true,
                toggle_minimap: false,
            }
        );
    }

    #[test]
    fn minimap_shortcut_requests_only_the_minimap_action() {
        let actions = detached_shortcut_actions(&[primary_shift_press(Key::M)], &AppShortcuts::default());

        assert_eq!(
            actions,
            DetachedShortcutActions {
                fit_workspace: false,
                toggle_minimap: true,
            }
        );
    }

    #[test]
    fn unrelated_keys_request_nothing() {
        let actions = detached_shortcut_actions(&[primary_shift_press(Key::Q)], &AppShortcuts::default());

        assert_eq!(actions, DetachedShortcutActions::default());
    }

    #[test]
    fn both_shortcuts_in_one_batch_request_both_actions() {
        let events = [primary_shift_press(Key::Num9), primary_shift_press(Key::M)];

        let actions = detached_shortcut_actions(&events, &AppShortcuts::default());

        assert_eq!(
            actions,
            DetachedShortcutActions {
                fit_workspace: true,
                toggle_minimap: true,
            }
        );
    }

    #[test]
    fn plain_keys_without_the_primary_modifier_request_nothing() {
        let events = [
            Event::Key {
                key: Key::Num9,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            },
            Event::Key {
                key: Key::M,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            },
        ];

        let actions = detached_shortcut_actions(&events, &AppShortcuts::default());

        assert_eq!(actions, DetachedShortcutActions::default());
    }
}
