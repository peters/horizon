//! A panel that follows a terminal of another computer.
//!
//! A follow panel holds a terminal that shows a stream, and no program of this
//! computer belongs to it. So nothing that Horizon does for a program that it
//! started may run for it. Two things make that sure. The panel is made here from
//! its content alone and never by the spawn of a panel, so it gets no holder, no
//! session binding, no agent identity, no plugin folder and no working folder,
//! and its kind is the shell, which no agent rule matches. The places that act on
//! a program of a panel later ask [`Panel::is_follower`] and do nothing for one:
//! a restart, the end of a program, the working folder of a workspace and the
//! saved session.

use crate::editor::PanelContent;
use crate::error::Result;
use crate::terminal::{FollowFeed, FollowSpec, Terminal};
use crate::workspace::WorkspaceId;

use super::{Panel, PanelId, PanelKind, current_unix_millis};

impl Panel {
    /// Opens the panel that shows the terminal of `spec`.
    ///
    /// # Errors
    ///
    /// Returns why the terminal of the follower cannot start.
    pub(crate) fn follow(id: PanelId, workspace_id: WorkspaceId, spec: FollowSpec) -> Result<Self> {
        let (terminal, _feed) = Terminal::spawn_follow(spec)?;
        let mut panel = Self::from_content(id, workspace_id, PanelKind::Shell, PanelContent::Terminal(terminal));
        if let Some(spec) = panel.follow_spec() {
            panel.title = if spec.title.is_empty() {
                spec.kind.display_name().to_owned()
            } else {
                spec.title.clone()
            };
        }
        // A local id of its own, as every panel of a session has one.
        panel.local_id = crate::runtime_state::new_local_id();
        panel.launched_at_millis = current_unix_millis();
        Ok(panel)
    }

    /// The terminal that this panel follows, or `None` for every other panel.
    #[must_use]
    pub fn follow_spec(&self) -> Option<&FollowSpec> {
        self.terminal()?.follow_spec()
    }

    /// The feed that gives this follow panel its stream and takes what the
    /// person typed. Every answer feeds the same panel.
    #[must_use]
    pub fn follow_feed(&self) -> Option<FollowFeed> {
        self.terminal()?.follow_feed()
    }

    /// Whether this panel follows a terminal of another computer, so no program
    /// of this computer belongs to it.
    #[must_use]
    pub fn is_follower(&self) -> bool {
        self.follow_spec().is_some()
    }

    /// The line under the grid of a follow panel whose stream ended.
    #[must_use]
    pub fn follow_ended_line(&self) -> Option<String> {
        let reason = self.terminal()?.follow_ended()?;
        let ended = if reason.is_empty() {
            "The stream of this terminal ended.".to_owned()
        } else {
            format!("The stream of this terminal ended: {reason}.")
        };
        Some(format!("{ended} Text typed here is not sent."))
    }

    /// The kind whose rules read the title and the status reports of the program
    /// of this panel: the kind of the panel that a follower shows.
    pub(super) fn claims_kind(&self) -> PanelKind {
        self.follow_spec().map_or(self.kind, |spec| spec.kind)
    }
}

#[cfg(all(test, unix))]
mod tests;
