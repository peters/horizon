//! Input for a parked cloud member. Its placeholder runs with other terminal modes
//! than the remote program, such as bracketed paste, so the input is held as events
//! and encoded for the attached terminal once it is back.
use std::borrow::Cow;

use horizon_core::{CloudWait, Panel, PanelId};

use crate::input::TerminalInputEvent;

/// Held events beyond this are dropped; nobody types that much before an attach.
const MAX_HELD_EVENTS: usize = 4096;

#[derive(Default)]
pub(crate) struct HeldInput {
    panel: Option<PanelId>,
    events: Vec<TerminalInputEvent>,
}

impl HeldInput {
    /// The events that `panel` takes now. A parked member takes none and its input
    /// is held, also while its attach reconnects; once attached it takes the held
    /// events first. A stopped cloud drops them.
    pub(crate) fn route<'e>(
        &mut self,
        panel: &Panel,
        events: &'e [TerminalInputEvent],
    ) -> Cow<'e, [TerminalInputEvent]> {
        let held_here = self.panel == Some(panel.id);
        let waits = match panel.cloud_wait() {
            Some(CloudWait::Parked) => true,
            Some(CloudWait::Reconnecting) => held_here,
            _ => false,
        };
        if waits {
            if !held_here {
                self.panel = Some(panel.id);
                self.events.clear();
            }
            let room = MAX_HELD_EVENTS.saturating_sub(self.events.len());
            self.events
                .extend(events.iter().filter(|event| is_input(&event.event)).take(room).cloned());
            return Cow::Owned(Vec::new());
        }
        if !held_here {
            return Cow::Borrowed(events);
        }
        self.panel = None;
        let held = std::mem::take(&mut self.events);
        if panel.cloud_wait().is_some() {
            return Cow::Borrowed(events);
        }
        Cow::Owned(held.into_iter().chain(events.iter().cloned()).collect())
    }

    /// Drops the held input once its panel no longer has the focus.
    pub(crate) fn forget_unless_focused(&mut self, focused: Option<PanelId>) {
        if self.panel.is_some() && self.panel != focused {
            self.panel = None;
            self.events.clear();
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.events.len()
    }
}

fn is_input(event: &egui::Event) -> bool {
    matches!(
        event,
        egui::Event::Text(_)
            | egui::Event::Key { pressed: true, .. }
            | egui::Event::Paste(_)
            | egui::Event::Ime(egui::ImeEvent::Commit(_))
            | egui::Event::Copy
            | egui::Event::Cut
    )
}
