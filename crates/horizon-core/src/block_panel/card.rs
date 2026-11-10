//! What a card remembers between frames: the person's clicks and the row it
//! has selected.
//!
//! None of this is saved. A click and a selection belong to the moment the
//! person sees, so they live in the Horizon process; the document, its
//! revision and the owner are what a restart keeps.

use std::collections::{BTreeMap, BTreeSet};

use horizon_browser_control::block::{BlockDocument, Component, Selection};

use super::clicks::{ClickQueue, PendingClick};
use super::clock::updated_label;

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
    /// The footer label of the revision it was formatted for. A frame reuses it
    /// instead of formatting a clock time, so a card never builds the text it
    /// draws again.
    updated: Option<(u64, i64, String)>,
    /// The height the body content needed in the last pass, when one ran.
    measured_content_height: Option<f32>,
    /// The document revision whose measured height the host already applied.
    fitted_revision: Option<u64>,
    /// The person resized this card by hand, so its size outranks its content.
    hand_resized: bool,
}

impl CardRuntime {
    /// Records a press on `component_id` against the revision the person saw.
    /// Returns whether the card queued it; a full queue keeps the older
    /// clicks.
    pub fn press(&mut self, component_id: &str, revision: u64, document: &BlockDocument, now_millis: i64) -> bool {
        let selected = self.selected_ids(document);
        self.clicks.push(component_id, revision, selected, now_millis)
    }

    /// Selects `row_id` in the list of component `component`. Returns whether
    /// the component is a list this card lets the person select.
    pub fn select(&mut self, component: usize, row_id: &str, document: &BlockDocument) -> bool {
        let Some(Component::List {
            selection: Selection::Single,
            rows,
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

    /// Records the height the body of this card needed in the pass that drew
    /// it. A frame learns it only after the body laid its content out.
    pub fn observe_content_height(&mut self, height: f32) {
        self.measured_content_height = Some(height);
    }

    /// The content height of the last pass, when one drew this card.
    #[must_use]
    pub fn measured_content_height(&self) -> Option<f32> {
        self.measured_content_height
    }

    /// Whether the host must still give this card the height its content of
    /// `revision` needs. One fit per revision: a second pass over the same
    /// document never resizes the card again, so no frame loop starts.
    #[must_use]
    pub fn needs_content_fit(&self, revision: u64) -> bool {
        !self.hand_resized && self.measured_content_height.is_some() && self.fitted_revision != Some(revision)
    }

    /// Records that the host gave the card the height its content of
    /// `revision` needs.
    pub fn mark_content_fit(&mut self, revision: u64) {
        self.fitted_revision = Some(revision);
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
}

#[cfg(test)]
mod tests {
    use horizon_browser_control::block::{Button, Component, ListRow, Selection, Tone};

    use super::CardRuntime;

    fn rows(ids: &[&str]) -> Vec<ListRow> {
        ids.iter()
            .map(|id| ListRow {
                id: (*id).to_owned(),
                title: (*id).to_owned(),
                subtitle: None,
                tone: None,
                selected: false,
            })
            .collect()
    }

    fn document(row_ids: &[&str]) -> horizon_browser_control::block::BlockDocument {
        horizon_browser_control::block::BlockDocument {
            components: vec![
                Component::Heading {
                    text: "cloud-1".to_owned(),
                    subtitle: None,
                    badge: None,
                },
                Component::List {
                    selection: Selection::Single,
                    rows: rows(row_ids),
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
}
