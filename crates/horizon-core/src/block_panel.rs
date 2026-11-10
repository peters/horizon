//! What a card panel holds: one document and the host state around it.
//!
//! [`BlockPanelState`] is the saved part: the document, its revision and the
//! owning agent. [`CardRuntime`] is what the person does with it between
//! frames: clicks that wait for the agent and the row a list has selected.

use horizon_browser_control::block::BlockDocument;
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
        }
    }

    /// An empty document, which the panel shows as an empty card.
    #[must_use]
    pub fn empty_document() -> BlockDocument {
        BlockDocument { components: Vec::new() }
    }

    /// Store `document` and return the revision that the host assigned to it.
    pub fn replace_document(&mut self, document: BlockDocument) -> u64 {
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
            components: vec![Component::Heading {
                text: text.to_string(),
                subtitle: None,
                badge: None,
            }],
        }
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
