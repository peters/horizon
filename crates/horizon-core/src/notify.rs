//! One desktop notification for an agent panel that changed while the person was
//! in another window.
//!
//! The sidebar says which agent panel needs the person and which one finished
//! unseen. A person who works in another window does not see the sidebar, so
//! Horizon sends one notification for such a change, through a program of the
//! system.
//!
//! The name of a panel is untrusted text: an agent sets its own terminal title.
//! [`clean_name`] removes every control character, turns a line break into one
//! space and cuts the name, [`body`] makes one line of it, and the arguments of the program put
//! the text after `--`, so a name is text and never an option of a program. The
//! boundary page [`docs/review/trust-boundaries/desktop-notifications.md`] states
//! the whole rule.
//!
//! [`docs/review/trust-boundaries/desktop-notifications.md`]: ../../../docs/review/trust-boundaries/desktop-notifications.md

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::util::truncate_chars;

mod bounds;
mod sink;

pub use bounds::{Bounds, CAPACITY, GROUP, SPACING, WINDOW};
pub use sink::{
    BOUND, DesktopSink, Sink, System, arguments, find_program, find_program_in, missing_sentence, program_name,
};

/// The title of every notification that Horizon shows.
pub const TITLE: &str = "Horizon";

/// What Horizon says when the person has had the share of notifications that one
/// window of ten minutes holds.
pub const SILENCE: &str = "More agents wait. Open Horizon.";

/// The most characters of a name that a notification shows.
pub const MAX_NAME_CHARS: usize = 60;

/// The value of the setting that turns desktop notifications on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationSetting {
    /// Horizon sends no notification.
    Off,
    /// Horizon sends one notification when an agent asks for the person.
    NeedsYou,
    /// Horizon sends one notification when an agent asks for the person or
    /// finishes while the person is elsewhere.
    #[default]
    NeedsYouAndDone,
}

impl NotificationSetting {
    /// The value as the row of the settings shows it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::NeedsYou => "When an agent needs me",
            Self::NeedsYouAndDone => "When an agent needs me or is done",
        }
    }

    /// Whether the setting sends a notification at all.
    #[must_use]
    pub const fn speaks(self) -> bool {
        !matches!(self, Self::Off)
    }

    /// Whether the setting sends a notification for an agent that finished
    /// while the person looked elsewhere.
    #[must_use]
    pub const fn tells_done(self) -> bool {
        matches!(self, Self::NeedsYouAndDone)
    }

    /// Every value, in the order of the row of the settings.
    pub const VALUES: [Self; 3] = [Self::Off, Self::NeedsYou, Self::NeedsYouAndDone];
}

/// Why an agent panel asks for the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    /// The agent asked something, or its turn failed.
    NeedsYou,
    /// The turn ended while the person looked elsewhere.
    Done,
}

impl ChangeKind {
    /// The words that follow the name of a panel.
    #[must_use]
    pub const fn words(self) -> &'static str {
        match self {
            Self::NeedsYou => "needs you",
            Self::Done => "is done",
        }
    }
}

impl fmt::Display for ChangeKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.words())
    }
}

/// One change of an agent panel that the person did not see.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// The name of the panel, which an agent can set. Untrusted text.
    pub panel: String,
    /// The name of the workspace, which the person typed. Untrusted text.
    pub workspace: Option<String>,
    pub kind: ChangeKind,
}

/// What one notification says, after the names are clean.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
}

impl Notice {
    /// The notification of `body`, with the title of Horizon.
    #[must_use]
    pub fn new(body: impl Into<String>) -> Self {
        Self {
            title: TITLE.to_owned(),
            body: body.into(),
        }
    }
}

/// One name as a notification shows it: no control character, no line break, one
/// space for a run of whitespace, and at most [`MAX_NAME_CHARS`] characters.
///
/// The text is still the text of another program: this clean makes it one line of
/// printable characters and says nothing about what it means.
#[must_use]
pub fn clean_name(name: &str) -> String {
    let mut clean = String::with_capacity(name.len());
    let mut pending_space = false;
    for character in name.chars() {
        // A line break, a tab and every other space of a line become one space
        // between two words; a control character that is no space goes away.
        let separator = character.is_whitespace();
        if separator || character.is_control() {
            pending_space |= separator && !clean.is_empty();
            continue;
        }
        if pending_space {
            clean.push(' ');
            pending_space = false;
        }
        clean.push(character);
    }
    match truncate_chars(&clean, MAX_NAME_CHARS) {
        std::borrow::Cow::Borrowed(text) => text.to_owned(),
        std::borrow::Cow::Owned(text) => text,
    }
}

/// The one line that the notification of `changes` shows.
///
/// One change names its panel, and its workspace when the board holds more than
/// one workspace. Several changes become one count, because a person who is in
/// another window does not read three names at once.
#[must_use]
pub fn body(changes: &[Change], several_workspaces: bool) -> String {
    match changes {
        [] => String::new(),
        [one] => {
            let mut line = format!("{} {}", clean_name(&one.panel), one.kind.words());
            if several_workspaces && let Some(workspace) = &one.workspace {
                let workspace = clean_name(workspace);
                if !workspace.is_empty() {
                    line.push_str(" \u{00B7} ");
                    line.push_str(&workspace);
                }
            }
            line
        }
        many => count_line(many),
    }
}

/// The count of several changes: `2 agents need you`, `1 is done`, and both.
fn count_line(changes: &[Change]) -> String {
    let needs_you = changes
        .iter()
        .filter(|change| change.kind == ChangeKind::NeedsYou)
        .count();
    let done = changes.len() - needs_you;
    let mut parts = Vec::new();
    if needs_you > 0 {
        parts.push(match needs_you {
            1 => "1 agent needs you".to_owned(),
            count => format!("{count} agents need you"),
        });
    }
    if done > 0 {
        parts.push(match done {
            1 => "1 is done".to_owned(),
            count => format!("{count} are done"),
        });
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests;
