//! What a card panel holds: one document and the host state around it.
//!
//! [`BlockPanelState`] is the saved part: the document, its revision and the
//! owning agent. [`CardRuntime`] is what the person does with it between
//! frames: clicks that wait for the agent and the row a list has selected.

use horizon_browser_control::block::{BlockDocument, Component};
use serde::{Deserialize, Serialize};

mod card;
mod clicks;
mod clock;
mod wake;

pub use card::{CardFrames, CardRuntime};
pub use clicks::{CLICK_RETENTION_MILLIS, ClickQueue, MAX_PENDING_CLICKS, PendingClick};
pub use wake::{WAKE_LINE, WAKE_LINE_INTERVAL, WakeContext, WakeHold, WakeVerdict, wake_verdict};

/// One card: the document that the host draws, the revision it assigned and
/// the agent that owns it. Saved with the panel, so a restart brings it back.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct BlockPanelState {
    /// One line, from [`Self::normalize_title`].
    pub title: String,
    /// Components in draw order.
    pub document: BlockDocument,
    /// Assigned by the host: 1 on create, one more with each new document.
    pub revision: u64,
    /// The stable `horizon:<local_id>` of the owning agent panel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_actor: Option<String>,
    /// Wall-clock time of the last create or update, in milliseconds after the
    /// epoch: zero for a card that records none.
    #[serde(default)]
    pub updated_at_millis: i64,
    /// The callout whose answer the person pressed, as the document held it, so
    /// the sidebar stops saying that this card needs the person until another
    /// callout asks something else. A press is the person's answer to a question
    /// of the card, and the answer outlives the frame and the run that saw it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_callout: Option<Component>,
    /// Where the card stands among the panels of its workspace.
    pub place: CardPlace,
    /// The fold that the person chose. Without one, a finished card is folded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fold: Option<bool>,
}

/// Where a card stands among the panels of its workspace. A card of a board
/// that an older build saved is a tile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CardPlace {
    /// A tile as every panel: the preset of its workspace arranges it.
    #[default]
    Tile,
    /// In the column of the cards of its agent panel, which Horizon lays out.
    Column,
    /// Where the person put it. No preset and no column moves it.
    Free,
}

impl BlockPanelState {
    /// What a card is called when its creator supplies no title.
    pub const DEFAULT_TITLE: &'static str = "Card";
    /// The longest title kept, in characters.
    pub const MAX_TITLE_CHARS: usize = 80;

    /// A card with `document`, at revision 1.
    #[must_use]
    pub fn new(document: BlockDocument) -> Self {
        Self {
            title: Self::DEFAULT_TITLE.to_owned(),
            document,
            revision: 1,
            owner_actor: None,
            updated_at_millis: 0,
            answered_callout: None,
            place: CardPlace::Tile,
            fold: None,
        }
    }

    /// Whether the card is one line: as the person left it, or because its job is finished.
    #[must_use]
    pub fn folded(&self) -> bool {
        self.fold.unwrap_or(self.document.finished)
    }

    /// The one line of a folded card: its heading, or its title when it has no
    /// heading, and the words of the badge of that heading.
    #[must_use]
    pub fn folded_line(&self) -> Option<(&str, Option<&str>)> {
        let heading = self.document.components.iter().find_map(|component| match component {
            Component::Heading { text, badge, .. } => Some((text.as_str(), badge.as_ref().map(|badge| &*badge.text))),
            _ => None,
        });
        self.folded().then(|| heading.unwrap_or((&self.title, None)))
    }

    /// Whether the card stands in the column of the cards of the agent `actor`.
    #[must_use]
    pub fn stands_in_column_of(&self, actor: &str) -> bool {
        self.place == CardPlace::Column && self.owner_actor.as_deref() == Some(actor)
    }

    /// The callout whose answer the person pressed, as the document holds it.
    #[must_use]
    pub fn answered_callout(&self) -> Option<&Component> {
        self.answered_callout.as_ref()
    }

    /// Records the person's answer to `callout`, as the document held it.
    pub fn record_answer(&mut self, callout: Component) {
        self.answered_callout = Some(callout);
    }

    /// Whether the person pressed an answer of `callout`, as the document holds
    /// it now. A callout with another question or another answer is a new
    /// question, so the card asks the person again.
    #[must_use]
    pub fn pressed_answer_of(&self, callout: &Component) -> bool {
        self.answered_callout.as_ref() == Some(callout)
    }

    /// An empty document, which the panel shows as an empty card.
    #[must_use]
    pub fn empty_document() -> BlockDocument {
        BlockDocument {
            width: None,
            finished: false,
            components: Vec::new(),
        }
    }

    /// Store `document` and return the revision that the host assigned to it.
    pub fn replace_document(&mut self, document: BlockDocument) -> u64 {
        // A job that finishes now folds its card, whatever the person chose before.
        self.fold = self.fold.filter(|_| self.document.finished || !document.finished);
        self.document = document;
        self.revision = self.revision.saturating_add(1);
        self.revision
    }

    /// One line, trimmed, at most [`Self::MAX_TITLE_CHARS`] characters, never
    /// empty: the first line of `raw` without control characters.
    #[must_use]
    pub fn normalize_title(raw: &str) -> String {
        let line: String = raw
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .filter(|character| !character.is_control())
            .collect();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Self::DEFAULT_TITLE.to_owned();
        }
        trimmed.chars().take(Self::MAX_TITLE_CHARS).collect()
    }
}

impl Default for BlockPanelState {
    fn default() -> Self {
        Self::new(Self::empty_document())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use horizon_browser_control::block::{BlockDocument, Component};

    use super::BlockPanelState;
    use crate::{Panel, PanelId, PanelKind, PanelOptions, WorkspaceId};

    fn document(text: &str) -> BlockDocument {
        BlockDocument {
            width: None,
            finished: false,
            components: vec![Component::Heading {
                text: text.to_string(),
                subtitle: None,
                badge: None,
            }],
        }
    }

    #[test]
    fn a_finished_card_is_folded_until_the_person_opens_it_and_a_fold_of_the_person_stays() {
        let finished = |text: &str| BlockDocument {
            finished: true,
            ..document(text)
        };
        let mut state = BlockPanelState::new(document("Start the server"));
        assert!(
            !state.folded() && state.folded_line().is_none(),
            "a card opens unfolded"
        );

        let _ = state.replace_document(finished("Server ready"));
        assert_eq!(
            state.folded_line(),
            Some(("Server ready", None)),
            "a finished job folds"
        );
        state.fold = Some(false);
        let _ = state.replace_document(finished("Server ready"));
        assert!(!state.folded(), "an update folds no card that the person opened");

        let _ = state.replace_document(document("Start the server"));
        let _ = state.replace_document(finished("Server ready"));
        assert!(state.folded(), "a job that finishes again folds its card again");

        let mut open = BlockPanelState::new(document("Start the server"));
        open.fold = Some(true);
        let _ = open.replace_document(document("Still working"));
        assert!(open.folded(), "a card that is not finished stays as the person left it");
        let saved = serde_json::to_string(&open).expect("the saved card");
        let restored: BlockPanelState = serde_json::from_str(&saved).expect("the restored card");
        assert_eq!(restored.fold, Some(true), "the fold is saved with the card");
    }

    #[test]
    fn a_new_card_starts_at_revision_one_with_no_owner() {
        let state = BlockPanelState::new(document("Cloud"));

        assert_eq!(state.title, "Card");
        assert_eq!(state.revision, 1);
        assert_eq!(state.owner_actor, None);
        assert_eq!(state.document, document("Cloud"));
    }

    #[test]
    fn a_card_keeps_the_time_of_its_last_change_across_save_and_load() {
        let mut state = BlockPanelState::new(document("one"));
        assert_eq!(
            state.updated_at_millis, 0,
            "a new card records no time until the host stamps it"
        );

        state.updated_at_millis = 1_790_883_960_000;
        let saved = serde_json::to_string(&state).expect("the state saves");
        let loaded: BlockPanelState = serde_json::from_str(&saved).expect("the state loads");
        assert_eq!(loaded.updated_at_millis, 1_790_883_960_000);
        assert_eq!(loaded, state);

        let older = r#"{"title":"Card","document":{"components":[]},"revision":2}"#;
        let loaded: BlockPanelState = serde_json::from_str(older).expect("a card saved before the footer loads");
        assert_eq!(loaded.updated_at_millis, 0);
        assert_eq!(loaded.revision, 2);
    }

    #[test]
    fn a_replaced_document_takes_the_next_revision() {
        let mut state = BlockPanelState::new(document("one"));

        assert_eq!(state.replace_document(document("two")), 2);
        assert_eq!(state.replace_document(document("three")), 3);
        assert_eq!(state.document, document("three"));
    }

    #[test]
    fn a_title_is_one_trimmed_line_of_at_most_eighty_characters() {
        assert_eq!(BlockPanelState::normalize_title("  Cloud one  "), "Cloud one");
        assert_eq!(BlockPanelState::normalize_title("Cloud\nsecond line"), "Cloud");
        assert_eq!(BlockPanelState::normalize_title("Cloud\r\nsecond line"), "Cloud");
        assert_eq!(BlockPanelState::normalize_title("Cloud \u{0}one"), "Cloud one");
        assert_eq!(BlockPanelState::normalize_title("   "), "Card");
        assert_eq!(BlockPanelState::normalize_title(""), "Card");

        let long = "a".repeat(120);
        assert_eq!(BlockPanelState::normalize_title(&long), "a".repeat(80));
    }

    #[test]
    fn a_block_spawn_has_no_process_terminal_or_text_input() -> crate::Result<()> {
        let mut panel = Panel::spawn(
            PanelId(1),
            WorkspaceId(1),
            PanelOptions {
                kind: PanelKind::Block,
                name: Some("Cloud".to_string()),
                block: Some(BlockPanelState::new(document("Cloud"))),
                ..PanelOptions::default()
            },
        )?;

        assert_eq!(panel.kind, PanelKind::Block);
        assert_eq!(panel.title, "Cloud");
        assert!(panel.terminal().is_none());
        assert!(panel.browser().is_none());
        assert!(!panel.kind.is_agent());
        assert!(!panel.kind.accepts_text_input());
        assert_eq!(panel.kind.display_name(), "Card");
        assert!(panel.launch_command.is_none());
        assert_eq!(panel.block().map(|state| state.revision), Some(1));

        panel.write_input(b"must not reach a process");
        panel.restart()?;
        panel.request_shutdown();
        assert!(panel.wait_for_shutdown(Duration::ZERO));
        assert!(panel.shutdown_with_timeout(Duration::ZERO));
        assert!(panel.terminal().is_none());
        Ok(())
    }

    #[test]
    fn a_block_spawn_without_state_starts_an_empty_card() -> crate::Result<()> {
        let panel = Panel::spawn(
            PanelId(2),
            WorkspaceId(1),
            PanelOptions {
                kind: PanelKind::Block,
                ..PanelOptions::default()
            },
        )?;

        let state = panel.block().expect("a card panel always holds its state");
        assert_eq!(state.title, "Card");
        assert_eq!(state.revision, 1);
        assert!(state.document.components.is_empty());
        Ok(())
    }
}
