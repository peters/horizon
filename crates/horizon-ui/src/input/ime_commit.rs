use std::collections::HashSet;

use egui::{Event, ImeEvent, ViewportId};

/// Rewrites orphan IME commits into plain text events.
///
/// On X11 with an ibus XIM bridge (`XMODIFIERS=@im=ibus`), winit delivers
/// plain non-ASCII keystrokes (e.g. Norwegian æøå) as a bare `Ime::Commit`
/// with no preceding `Ime::Enabled`/`Preedit`. egui's `TextEdit` discards
/// such orphan commits because its stored IME cursor range is stale, so the
/// characters never appear. Rewriting orphan commits into `Event::Text`
/// routes them through the normal typing path while leaving genuine
/// composition sessions (CJK preedit flows) untouched.
#[derive(Default)]
pub(crate) struct ImeCommitNormalizer {
    composing: HashSet<ViewportId>,
}

impl ImeCommitNormalizer {
    pub(crate) fn normalize(&mut self, viewport_id: ViewportId, events: &mut [Event]) {
        for event in events {
            match event {
                // A non-empty preedit starts a composition session; an empty
                // preedit means the IME was dismissed.
                Event::Ime(ImeEvent::Preedit { text, .. }) => {
                    if text.is_empty() {
                        self.composing.remove(&viewport_id);
                    } else {
                        self.composing.insert(viewport_id);
                    }
                }
                Event::Ime(ImeEvent::Commit(text)) => {
                    let orphan_commit = !self.composing.remove(&viewport_id);
                    if orphan_commit {
                        *event = Event::Text(std::mem::take(text));
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use egui::{Event, ImeEvent, ViewportId};

    use super::ImeCommitNormalizer;

    fn commit(text: &str) -> Event {
        Event::Ime(ImeEvent::Commit(text.to_owned()))
    }

    fn preedit(text: &str) -> Event {
        Event::Ime(ImeEvent::Preedit {
            text: text.to_owned(),
            active_range_chars: None,
        })
    }

    #[test]
    fn orphan_commit_becomes_text_event() {
        let mut normalizer = ImeCommitNormalizer::default();
        let mut events = vec![preedit(""), commit("æ"), preedit("")];

        normalizer.normalize(ViewportId::ROOT, &mut events);

        assert_eq!(events[1], Event::Text("æ".to_owned()));
    }

    #[test]
    fn commit_after_enabled_stays_a_commit() {
        let mut normalizer = ImeCommitNormalizer::default();
        let mut events = vec![preedit("中"), commit("中")];

        normalizer.normalize(ViewportId::ROOT, &mut events);

        assert_eq!(events[1], commit("中"));
    }

    #[test]
    fn composition_state_spans_frames() {
        let mut normalizer = ImeCommitNormalizer::default();
        let mut first_frame = vec![preedit("中")];
        normalizer.normalize(ViewportId::ROOT, &mut first_frame);

        let mut second_frame = vec![commit("中")];
        normalizer.normalize(ViewportId::ROOT, &mut second_frame);

        assert_eq!(second_frame[0], commit("中"));
    }

    #[test]
    fn commit_ends_the_composition_session() {
        let mut normalizer = ImeCommitNormalizer::default();
        let mut events = vec![preedit("中"), commit("中"), commit("ø")];

        normalizer.normalize(ViewportId::ROOT, &mut events);

        assert_eq!(events[1], commit("中"));
        assert_eq!(events[2], Event::Text("ø".to_owned()));
    }

    #[test]
    fn composition_state_is_tracked_per_viewport() {
        let mut normalizer = ImeCommitNormalizer::default();
        let mut root_frame = vec![preedit("中")];
        normalizer.normalize(ViewportId::ROOT, &mut root_frame);

        let other = ViewportId::from_hash_of("detached");
        let mut other_frame = vec![commit("å")];
        normalizer.normalize(other, &mut other_frame);

        assert_eq!(other_frame[0], Event::Text("å".to_owned()));
    }
}
