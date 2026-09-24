use egui::{Context, Event};
use horizon_core::ShortcutBinding;

use super::HorizonApp;
use super::shortcuts::{event_uses_shortcut_key, shortcut_key_may_emit_text};

impl HorizonApp {
    pub(super) fn consume_navigation_key(&mut self, ctx: &Context, binding: ShortcutBinding) {
        self.hold_navigation_key(binding);
        self.filter_navigation_events(ctx, false);
    }

    /// Swallows the key's repeats from the next frame until its release, and
    /// leaves this frame's press to a consumer that has not run yet.
    pub(super) fn hold_navigation_key(&mut self, binding: ShortcutBinding) {
        self.held_navigation_keys.push(binding);
    }

    pub(super) fn filter_held_navigation_keys(&mut self, ctx: &Context) {
        self.filter_navigation_events(ctx, true);
    }

    fn filter_navigation_events(&mut self, ctx: &Context, accept_fresh_press: bool) {
        ctx.input_mut(|input| {
            let mut correlated_text = false;
            // Own the gesture through destination-view changes and focus
            // loss, but never swallow a fresh press after a missing release.
            input.events.retain(|event| {
                let consume_text = correlated_text && matches!(event, Event::Text(_));
                correlated_text = false;
                if consume_text {
                    return false;
                }
                let Some(index) = self
                    .held_navigation_keys
                    .iter()
                    .position(|held| event_uses_shortcut_key(event, *held))
                else {
                    return true;
                };
                if accept_fresh_press
                    && matches!(
                        event,
                        Event::Key {
                            pressed: true,
                            repeat: false,
                            ..
                        }
                    )
                {
                    self.held_navigation_keys.remove(index);
                    return true;
                }
                if matches!(event, Event::Key { pressed: false, .. }) {
                    self.held_navigation_keys.remove(index);
                } else {
                    correlated_text = shortcut_key_may_emit_text(self.held_navigation_keys[index].key);
                }
                false
            });
        });
    }
}

#[cfg(test)]
mod tests;
