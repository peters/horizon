//! What a card remembers between frames: the person's clicks and the row it
//! has selected.
//!
//! A click and a selection belong to the moment the person sees, so they live in
//! the Horizon process. The document, its revision, the owner and the callout
//! whose answer the person pressed are what a restart keeps; the answer moves to
//! the saved card as soon as a frame settles, because the sidebar reads it there
//! after a restart.

use std::collections::{BTreeMap, BTreeSet};

use horizon_browser_control::block::{BlockDocument, Component, Selection, link_host};

use super::clicks::{ClickQueue, PendingClick};
use super::clock::updated_label;

/// The callout of `document` that holds the button or the answer `component_id`,
/// so a press on an answer of a callout is the person's answer to that question.
fn answered_callout<'a>(document: &'a BlockDocument, component_id: &str) -> Option<&'a Component> {
    document.components.iter().find(|component| match component {
        Component::Callout { buttons, .. } => buttons.iter().any(|button| button.id == component_id),
        _ => false,
    })
}

/// The frames that Horizon adds above the body of a card, outside the document
/// its agent wrote.
///
/// Each frame takes room that the content of a card needs, so a change of this
/// set changes the height the card fits, exactly like a new document does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CardFrames {
    /// The card says that its agent is not in the workspace.
    pub agent_gone: bool,
    /// The card says that a click was refused while the queue stayed full.
    pub clicks_refused: bool,
    /// A link of the card says that its page did not open.
    pub link_refused: bool,
}

/// A card's runtime state: the clicks it holds for its agent, the row each
/// list has selected, the height its body measured, and whether a wake line
/// already went out.
#[derive(Clone, Debug, Default)]
pub struct CardRuntime {
    clicks: ClickQueue,
    /// The selected row of a list, by its component index in the document.
    selection: BTreeMap<usize, String>,
    /// The lists whose row the person selected, by component index. A row that
    /// the document selects by default is not a choice of the person.
    chosen: BTreeSet<usize>,
    /// A wake line already went to the owner for this streak of clicks.
    wake_sent: bool,
    /// The label of a button that the person pressed since the host read it,
    /// which the host names in the conversation of an orchestrator.
    press_note: Option<String>,
    /// The footer label of the revision it was formatted for. A frame reuses it
    /// instead of formatting a clock time, so a card never builds the text it
    /// draws again.
    updated: Option<(u64, i64, String)>,
    /// The host drawn under each link, by its component index, for the revision
    /// it was read from. A frame reuses the host it resolved instead of reading
    /// the URL again; a new document reads its links again.
    link_hosts: Option<(u64, BTreeMap<usize, String>)>,
    /// Why the page of a link did not open, by the address of that link, for the
    /// revision that the person pressed. A new document drops them, because the
    /// person reads the document that replaced the one they pressed.
    link_failures: Option<(u64, BTreeMap<String, String>)>,
    /// The height the body content needed in the last pass, when one ran.
    measured_content_height: Option<f32>,
    /// The frames the last pass drew above the body.
    frames: CardFrames,
    /// The document revision and frames whose measured height the host already
    /// applied.
    fitted: Option<(u64, CardFrames)>,
    /// The person resized this card by hand, so its size outranks its content.
    hand_resized: bool,
    /// The callout whose answer the person pressed, as the document held it, so
    /// the row of this card stops saying that it needs the person until another
    /// callout asks something else.
    answered_callout: Option<Component>,
}

impl CardRuntime {
    /// Records a press on `component_id` against the revision the person saw.
    /// Returns whether the card queued it; a full queue keeps the older
    /// clicks.
    pub fn press(&mut self, component_id: &str, revision: u64, document: &BlockDocument, now_millis: i64) -> bool {
        if let Some(callout) = answered_callout(document, component_id) {
            self.answered_callout = Some(callout.clone());
        }
        let selected = self.selected_ids(document);
        self.clicks.push(component_id, revision, selected, now_millis)
    }

    /// Whether the person pressed an answer of `callout`, as the document holds
    /// it now. A callout with another question or another answer is a new
    /// question, so the card asks the person again.
    #[must_use]
    pub fn pressed_answer_of(&self, callout: &Component) -> bool {
        self.answered_callout.as_ref() == Some(callout)
    }

    /// The callout whose answer the person pressed, as this card holds it now.
    #[must_use]
    pub fn answered_callout(&self) -> Option<&Component> {
        self.answered_callout.as_ref()
    }

    /// Records an answer that the saved card already held, so the live card of a
    /// restored run starts quiet.
    pub fn record_answer(&mut self, callout: Component) {
        self.answered_callout = Some(callout);
    }

    /// Selects `row_id` in the list of component `component`. Returns whether
    /// the component is a list this card lets the person select.
    pub fn select(&mut self, component: usize, row_id: &str, document: &BlockDocument) -> bool {
        let Some(Component::List {
            selection: Selection::Single,
            rows,
            ..
        }) = document.components.get(component)
        else {
            return false;
        };
        if !rows.iter().any(|row| row.id == row_id) {
            return false;
        }
        self.selection.insert(component, row_id.to_owned());
        self.chosen.insert(component);
        true
    }

    /// The footer label, `updated HH:MM`, of the document at `revision` whose
    /// last change was `updated_at_millis`. The clock time is formatted when the
    /// revision or the time changes, never per frame.
    pub fn updated_label(&mut self, revision: u64, updated_at_millis: i64) -> &str {
        let stale = self
            .updated
            .as_ref()
            .is_none_or(|(at_revision, at_millis, _)| *at_revision != revision || *at_millis != updated_at_millis);
        if stale {
            self.updated = Some((revision, updated_at_millis, updated_label(updated_at_millis)));
        }
        self.updated.as_ref().map_or("", |(_, _, label)| label.as_str())
    }

    /// The host that the card draws under the link of component `component`,
    /// read from `document` at `revision`.
    ///
    /// The hosts of a document are read once, when it arrives, and not on every
    /// frame: the address of a link is the agent's text, so the host a person
    /// reads under it is a normalized address that costs a parse.
    pub fn link_host(&mut self, revision: u64, component: usize, document: &BlockDocument) -> Option<&str> {
        if self.link_hosts.as_ref().is_none_or(|(at, _)| *at != revision) {
            let hosts = document
                .components
                .iter()
                .enumerate()
                .filter_map(|(index, component)| match component {
                    Component::Link { url, .. } => link_host(url).map(|host| (index, host)),
                    _ => None,
                })
                .collect();
            self.link_hosts = Some((revision, hosts));
        }
        self.link_hosts
            .as_ref()
            .and_then(|(_, hosts)| hosts.get(&component))
            .map(String::as_str)
    }

    /// Records that the press on the link of `url` opened nothing, and why, for
    /// the document at `revision`. The card draws the reason under that link
    /// until the person opens it or a new document arrives.
    pub fn fail_link(&mut self, revision: u64, url: &str, reason: &str) {
        let failures = match &mut self.link_failures {
            Some((at, failures)) if *at == revision => failures,
            slot => &mut slot.insert((revision, BTreeMap::new())).1,
        };
        failures.insert(url.to_owned(), reason.to_owned());
    }

    /// Why the page of the link of `url` did not open, as the card at `revision`
    /// says it. A newer document says nothing about the press of an older one.
    #[must_use]
    pub fn link_failure(&self, revision: u64, url: &str) -> Option<&str> {
        self.link_failures
            .as_ref()
            .filter(|(at, _)| *at == revision)
            .and_then(|(_, failures)| failures.get(url))
            .map(String::as_str)
    }

    /// Forgets the reason of `url`: its next press opened the page.
    pub fn clear_link_failure(&mut self, url: &str) {
        if let Some((_, failures)) = &mut self.link_failures {
            failures.remove(url);
        }
    }

    /// Whether the card at `revision` says that a link did not open.
    #[must_use]
    pub fn has_link_failure(&self, revision: u64) -> bool {
        self.link_failures
            .as_ref()
            .is_some_and(|(at, failures)| *at == revision && !failures.is_empty())
    }

    /// Records the height the body of this card needed in the pass that drew
    /// it. A frame learns it only after the body laid its content out.
    pub fn observe_content_height(&mut self, height: f32) {
        self.measured_content_height = Some(height);
    }

    /// Records the frames that the pass drew above the body of this card.
    pub fn observe_frames(&mut self, frames: CardFrames) {
        self.frames = frames;
    }

    /// The frames that the last pass drew above the body, as last drawn.
    #[must_use]
    pub fn frames(&self) -> CardFrames {
        self.frames
    }

    /// The content height of the last pass, when one drew this card.
    #[must_use]
    pub fn measured_content_height(&self) -> Option<f32> {
        self.measured_content_height
    }

    /// Whether the host must still give this card the height its content of
    /// `revision` needs, with the frames of the last pass above it. One fit per
    /// document and set of frames: a second pass that draws the same ones never
    /// resizes the card again, so no frame loop starts.
    #[must_use]
    pub fn needs_content_fit(&self, revision: u64) -> bool {
        !self.hand_resized && self.measured_content_height.is_some() && self.fitted != Some((revision, self.frames))
    }

    /// Forgets the last fit: a card that opens measures its content again.
    pub fn forget_content_fit(&mut self) {
        self.measured_content_height = None;
        self.fitted = None;
    }

    /// Records that the host gave the card the height its content and its
    /// frames of `revision` need.
    pub fn mark_content_fit(&mut self, revision: u64) {
        self.fitted = Some((revision, self.frames));
    }

    /// Whether the person resized this card by hand.
    #[must_use]
    pub fn hand_resized(&self) -> bool {
        self.hand_resized
    }

    /// Records that the person resized this card by hand: from then on the
    /// card keeps the size that person chose, whatever its document needs.
    pub fn mark_hand_resized(&mut self) {
        self.hand_resized = true;
    }

    /// The row selected in component `component`.
    #[must_use]
    pub fn selected(&self, component: usize) -> Option<&str> {
        self.selection.get(&component).map(String::as_str)
    }

    /// The selected row ids in draw order: what a click reports as `selected`.
    #[must_use]
    pub fn selected_ids(&self, document: &BlockDocument) -> Vec<String> {
        document
            .components
            .iter()
            .enumerate()
            .filter_map(|(index, component)| {
                let Component::List {
                    selection: Selection::Single,
                    rows,
                    ..
                } = component
                else {
                    return None;
                };
                let selected = self.selection.get(&index)?;
                rows.iter().any(|row| row.id == *selected).then(|| selected.clone())
            })
            .collect()
    }

    /// Gives every list that the person has not selected in the row its
    /// document selects by default, so a click reports that row until the
    /// person chooses another one.
    ///
    /// A selection the person made wins while its row is still drawn: a
    /// document never moves the selection away from under the person's hand. A
    /// default of an older document is not a choice of the person, so a new
    /// document may select another row.
    pub fn apply_default_selection(&mut self, document: &BlockDocument) {
        for (index, component) in document.components.iter().enumerate() {
            let Component::List {
                selection: Selection::Single,
                rows,
                ..
            } = component
            else {
                continue;
            };
            let chosen = self.chosen.contains(&index)
                && self
                    .selection
                    .get(&index)
                    .is_some_and(|selected| rows.iter().any(|row| row.id == *selected));
            if chosen {
                continue;
            }
            match rows.iter().find(|row| row.selected) {
                Some(row) => {
                    self.selection.insert(index, row.id.clone());
                }
                None => {
                    self.selection.remove(&index);
                }
            }
        }
        self.chosen.retain(|index| self.selection.contains_key(index));
    }

    /// Drops the selections of rows that a new document no longer holds.
    pub fn retain_selection(&mut self, document: &BlockDocument) {
        self.selection.retain(|index, selected| {
            let Some(Component::List {
                selection: Selection::Single,
                rows,
                ..
            }) = document.components.get(*index)
            else {
                return false;
            };
            rows.iter().any(|row| row.id == *selected)
        });
        self.chosen.retain(|index| self.selection.contains_key(index));
    }

    /// The oldest click that still waits, removed: a click reaches an agent at
    /// most once.
    pub fn take_click(&mut self, now_millis: i64) -> Option<PendingClick> {
        self.clicks.take(now_millis)
    }

    /// The time of the oldest click that still waits, after dropping the ones
    /// that aged out.
    pub fn oldest_click_at(&mut self, now_millis: i64) -> Option<i64> {
        self.clicks.oldest_at(now_millis)
    }

    /// How many clicks wait for the agent, after dropping the ones that aged
    /// out.
    pub fn pending_clicks(&mut self, now_millis: i64) -> usize {
        self.clicks.pending(now_millis)
    }

    /// How many clicks wait for the agent, as last pruned.
    #[must_use]
    pub fn queued(&self) -> usize {
        self.clicks.len()
    }

    /// Whether a click was refused while the queue stayed full, so the card
    /// says that too many clicks wait.
    pub fn refused_notice(&mut self, now_millis: i64) -> bool {
        self.clicks.refused_notice(now_millis)
    }

    /// Whether a wake line already went out for this streak of clicks.
    #[must_use]
    pub fn wake_sent(&self) -> bool {
        self.wake_sent
    }

    pub fn mark_wake_sent(&mut self) {
        self.wake_sent = true;
    }

    /// Starts a new streak: a waiter existed, so the owner is listening again
    /// for the next click.
    pub fn reset_wake_streak(&mut self) {
        self.wake_sent = false;
    }

    /// Records the label of the button that the person just pressed, which the
    /// host names in the conversation of the workspace. The host takes it one
    /// time, so a button that the person presses twice makes two notes.
    pub fn note_press(&mut self, label: &str) {
        self.press_note = Some(label.to_owned());
    }

    /// The label of the press that the host did not name yet, removed: one press
    /// gives one line of the conversation.
    pub fn take_press_note(&mut self) -> Option<String> {
        self.press_note.take()
    }
}

#[cfg(test)]
mod tests {
    use horizon_browser_control::block::{Button, Component, ListLayout, ListRow, Selection, Tone};

    use super::{CardFrames, CardRuntime};

    fn rows(ids: &[&str]) -> Vec<ListRow> {
        ids.iter()
            .map(|id| ListRow {
                id: (*id).to_owned(),
                title: (*id).to_owned(),
                subtitle: None,
                tone: None,
                selected: false,
                value: None,
                badge: None,
                disabled: false,
            })
            .collect()
    }

    fn document(row_ids: &[&str]) -> horizon_browser_control::block::BlockDocument {
        horizon_browser_control::block::BlockDocument {
            width: None,
            finished: false,
            components: vec![
                Component::Heading {
                    text: "cloud-1".to_owned(),
                    subtitle: None,
                    badge: None,
                },
                Component::List {
                    selection: Selection::Single,
                    rows: rows(row_ids),
                    layout: ListLayout::Rows,
                },
                Component::Buttons {
                    items: vec![Button {
                        id: "continue".to_owned(),
                        label: "Continue".to_owned(),
                        tone: Tone::Primary,
                    }],
                },
            ],
        }
    }

    #[test]
    fn a_pressed_button_reports_the_selected_rows_in_document_order() {
        let card = document(&["size.cx22", "size.cx32"]);
        let mut runtime = CardRuntime::default();
        assert!(runtime.select(1, "size.cx32", &card));
        assert!(runtime.press("continue", 4, &card, 1_000), "a press is queued");

        let click = runtime.take_click(1_000).expect("click");
        assert_eq!(click.component_id, "continue");
        assert_eq!(click.revision, 4);
        assert_eq!(click.selected, vec!["size.cx32".to_owned()]);
        assert_eq!(runtime.selected_ids(&card), vec!["size.cx32".to_owned()]);
    }

    #[test]
    fn only_a_single_selection_list_takes_a_selection() {
        let mut card = document(&["a"]);
        card.components.push(Component::List {
            selection: Selection::None,
            rows: rows(&["b"]),
            layout: ListLayout::Rows,
        });
        let mut runtime = CardRuntime::default();

        assert!(runtime.select(1, "a", &card));
        assert!(
            !runtime.select(1, "missing", &card),
            "a row that is not there is not selected"
        );
        assert_eq!(runtime.selected(1), Some("a"));
        assert!(
            !runtime.select(2, "b", &card),
            "a list the person cannot select takes nothing"
        );
        assert!(!runtime.select(0, "a", &card), "a heading is not a list");
        assert_eq!(runtime.selected_ids(&card), vec!["a".to_owned()]);
    }

    #[test]
    fn an_update_that_removes_the_selected_row_clears_the_selection() {
        let mut runtime = CardRuntime::default();
        let before = document(&["size.cx22", "size.cx32"]);
        assert!(runtime.select(1, "size.cx32", &before));

        runtime.retain_selection(&document(&["size.cx22", "size.cx32"]));
        assert_eq!(
            runtime.selected(1),
            Some("size.cx32"),
            "a row that stays keeps its selection"
        );

        runtime.retain_selection(&document(&["size.cx22"]));
        assert_eq!(runtime.selected(1), None, "a removed row clears the selection");
        assert!(runtime.selected_ids(&document(&["size.cx22"])).is_empty());
    }

    #[test]
    fn a_card_fits_its_content_once_per_revision_until_a_person_resizes_it() {
        let mut runtime = CardRuntime::default();
        assert!(!runtime.needs_content_fit(1), "no pass measured the content yet");

        runtime.observe_content_height(415.0);
        assert_eq!(runtime.measured_content_height(), Some(415.0));
        assert!(runtime.needs_content_fit(1), "the first pass asks for one fit");

        runtime.mark_content_fit(1);
        assert!(!runtime.needs_content_fit(1), "the same revision never fits twice");
        assert!(runtime.needs_content_fit(2), "a new document fits again");

        runtime.mark_hand_resized();
        assert!(runtime.hand_resized());
        assert!(
            !runtime.needs_content_fit(3),
            "a card that the person resized by hand keeps its size"
        );
    }

    #[test]
    fn a_frame_of_the_host_fits_the_card_again_and_only_once() {
        let mut runtime = CardRuntime::default();
        runtime.observe_content_height(415.0);
        runtime.mark_content_fit(1);
        assert_eq!(runtime.frames(), CardFrames::default(), "a live card draws no frame");
        assert!(!runtime.needs_content_fit(1), "a settled card stays put");

        let gone = CardFrames {
            agent_gone: true,
            clicks_refused: false,
            link_refused: false,
        };
        runtime.observe_frames(gone);
        assert!(
            runtime.needs_content_fit(1),
            "the frame of a gone agent needs room, so the card fits again"
        );
        runtime.mark_content_fit(1);
        assert!(
            !runtime.needs_content_fit(1),
            "the same document and the same frames never fit twice"
        );

        runtime.observe_frames(CardFrames {
            agent_gone: true,
            clicks_refused: true,
            link_refused: false,
        });
        assert!(
            runtime.needs_content_fit(1),
            "a full queue adds a second frame, so the card fits again"
        );
        runtime.mark_content_fit(1);
        runtime.observe_frames(gone);
        assert!(
            runtime.needs_content_fit(1),
            "the agent back removes its frame, so the card shrinks again"
        );

        runtime.mark_hand_resized();
        assert!(
            !runtime.needs_content_fit(1),
            "a card that the person resized by hand keeps its size"
        );
    }

    #[test]
    fn a_list_without_a_selection_takes_the_row_its_document_selects() {
        let mut runtime = CardRuntime::default();
        let mut card = document(&["size.cx22", "size.cx32"]);
        if let Some(Component::List { rows, .. }) = card.components.get_mut(1) {
            rows[1].selected = true;
        }

        runtime.apply_default_selection(&card);
        assert_eq!(runtime.selected(1), Some("size.cx32"), "the document selects a row");

        assert!(runtime.select(1, "size.cx22", &card), "the person selects another row");
        runtime.apply_default_selection(&card);
        assert_eq!(
            runtime.selected(1),
            Some("size.cx22"),
            "the person's selection wins over the default"
        );

        // An update that drops the person's row falls back to the default.
        let mut updated = card.clone();
        if let Some(Component::List { rows, .. }) = updated.components.get_mut(1) {
            rows.retain(|row| row.id != "size.cx22");
        }
        runtime.retain_selection(&updated);
        runtime.apply_default_selection(&updated);
        assert_eq!(runtime.selected(1), Some("size.cx32"));
        assert!(runtime.press("continue", 1, &updated, 1_000));
        let click = runtime.take_click(1_000).expect("a click");
        assert_eq!(click.selected, vec!["size.cx32".to_owned()]);
    }

    #[test]
    fn a_new_default_replaces_a_default_of_an_older_document() {
        let mut runtime = CardRuntime::default();
        runtime.apply_default_selection(&document(&["size.cx22", "size.cx32"]));
        assert_eq!(runtime.selected(1), None, "no row is selected by default");

        let mut card = document(&["size.cx22", "size.cx32"]);
        if let Some(Component::List { rows, .. }) = card.components.get_mut(1) {
            rows[0].selected = true;
        }
        runtime.apply_default_selection(&card);
        assert_eq!(runtime.selected(1), Some("size.cx22"));

        let mut next = card.clone();
        if let Some(Component::List { rows, .. }) = next.components.get_mut(1) {
            rows[0].selected = false;
            rows[1].selected = true;
        }
        runtime.retain_selection(&next);
        runtime.apply_default_selection(&next);
        assert_eq!(
            runtime.selected(1),
            Some("size.cx32"),
            "the person did not choose the old default, so the new document decides"
        );
    }

    #[test]
    fn a_wake_streak_survives_clicks_and_restarts_when_a_waiter_exists() {
        let card = document(&["size.cx22"]);
        let mut runtime = CardRuntime::default();
        assert!(!runtime.wake_sent());
        runtime.mark_wake_sent();
        assert!(runtime.wake_sent());
        assert!(runtime.press("continue", 1, &card, 1_000));
        assert!(
            runtime.wake_sent(),
            "a second click of the same streak does not wake again"
        );
        runtime.reset_wake_streak();
        assert!(!runtime.wake_sent());
    }

    #[test]
    fn one_press_gives_one_note_to_the_host() {
        let mut runtime = CardRuntime::default();
        assert!(runtime.take_press_note().is_none(), "no press yet");
        runtime.note_press("Go");
        assert_eq!(runtime.take_press_note(), Some("Go".to_owned()));
        assert!(
            runtime.take_press_note().is_none(),
            "the label of a press reaches the host one time"
        );
    }
}
