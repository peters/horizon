//! Why a panel that the person asked for opened nothing.
//!
//! A panel creation can fail in the engine, in the PTY or in the machine of a
//! placed workspace, and every one of those reasons used to reach the log only.
//! The person who pressed the button is the one who has to act on it, so the
//! reason goes to the sidebar notice in the tool's own words, and a browser
//! engine whose panel ends with an error says it before the cleanup takes its
//! panel away.

use horizon_core::PanelId;
use horizon_core::browser::BrowserStatus;

/// The card link that opened a browser panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct LinkOrigin {
    /// The card whose link the person pressed.
    pub(in crate::app) card: PanelId,
    /// The address of that link, as the document names it.
    pub(in crate::app) url: String,
}

/// One browser panel that an action of the person opened.
#[derive(Clone, Debug)]
struct PersonBrowserCreate {
    panel: PanelId,
    /// The card link that opened the panel, when a link opened it.
    link: Option<LinkOrigin>,
}

/// A browser panel of the person that ended, and the card link that opened it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct EndedPersonBrowser {
    /// The card link that opened the panel, when a link opened it.
    pub(in crate::app) link: Option<LinkOrigin>,
}

/// The browser panels that the person opened and that can still end, with the
/// card link that opened each one.
///
/// An agent's create answers the agent with its own typed failure, so it is not
/// here: the person never sees it. A record stays until the cleanup reads the
/// end of its panel, so one panel says one reason.
#[derive(Default)]
pub(in crate::app) struct PersonBrowserCreates {
    panels: Vec<PersonBrowserCreate>,
}

impl PersonBrowserCreates {
    /// Remembers that the person's action opened `panel`.
    pub(in crate::app) fn note(&mut self, panel: PanelId, link: Option<LinkOrigin>) {
        self.panels.retain(|create| create.panel != panel);
        self.panels.push(PersonBrowserCreate { panel, link });
    }

    /// Takes the record of `panel`, so one end of a panel says one reason.
    pub(in crate::app) fn take(&mut self, panel: PanelId) -> Option<EndedPersonBrowser> {
        let index = self.panels.iter().position(|create| create.panel == panel)?;
        Some(EndedPersonBrowser {
            link: self.panels.remove(index).link,
        })
    }

    /// Drops the records of panels that are gone, so a closed panel is no half
    /// memory of the window it opened in.
    pub(in crate::app) fn retain(&mut self, mut keep: impl FnMut(PanelId) -> bool) {
        self.panels.retain(|create| keep(create.panel));
    }

    /// Whether no panel waits, so a frame has nothing to clean up.
    pub(in crate::app) fn is_empty(&self) -> bool {
        self.panels.is_empty()
    }
}

/// Why the browser engine of an ended panel could not start, in the words of
/// the engine, or `None` for a panel that stopped without an error.
pub(in crate::app) fn engine_failure_reason(status: &BrowserStatus) -> Option<String> {
    match status {
        BrowserStatus::Error { message } => Some(message.clone()),
        BrowserStatus::Stopped { code: Some(code) } if *code != 0 => {
            Some(format!("the browser engine exited with code {code}"))
        }
        BrowserStatus::Starting | BrowserStatus::Ready | BrowserStatus::Stopped { .. } => None,
    }
}

/// The notice of a panel that the person asked for and that opened nothing.
pub(in crate::app) fn panel_creation_notice(kind: horizon_core::PanelKind, reason: &str) -> String {
    format!("{} panel: {reason}", kind.display_name())
}

#[cfg(test)]
mod tests {
    use super::{LinkOrigin, PersonBrowserCreates, engine_failure_reason, panel_creation_notice};
    use horizon_core::PanelId;
    use horizon_core::browser::BrowserStatus;

    #[test]
    fn a_notice_names_the_kind_of_panel_and_the_reason_of_the_tool() {
        assert_eq!(
            panel_creation_notice(
                horizon_core::PanelKind::Browser,
                "no Chrome/Chromium binary found (set browser.command in the config)"
            ),
            "Browser panel: no Chrome/Chromium binary found (set browser.command in the config)"
        );
        assert_eq!(
            panel_creation_notice(horizon_core::PanelKind::Shell, "no PTY for this process"),
            "Shell panel: no PTY for this process"
        );
    }

    #[test]
    fn an_engine_that_ends_with_an_error_says_its_own_words() {
        assert_eq!(
            engine_failure_reason(&BrowserStatus::Error {
                message: "no Chrome/Chromium binary found".into()
            })
            .as_deref(),
            Some("no Chrome/Chromium binary found")
        );
        assert_eq!(
            engine_failure_reason(&BrowserStatus::Stopped { code: Some(2) }).as_deref(),
            Some("the browser engine exited with code 2")
        );
        assert_eq!(engine_failure_reason(&BrowserStatus::Stopped { code: None }), None);
        assert_eq!(engine_failure_reason(&BrowserStatus::Ready), None);
    }

    #[test]
    fn a_panel_that_ends_keeps_its_link_and_its_reason_is_taken_once() {
        let mut creates = PersonBrowserCreates::default();
        let panel = PanelId(7);
        let link = LinkOrigin {
            card: PanelId(3),
            url: "https://cloud.example.test/sessions/7".into(),
        };
        creates.note(panel, Some(link.clone()));

        let ended = creates.take(panel).expect("the record of the panel");

        assert_eq!(ended.link, Some(link));
        assert_eq!(creates.take(panel), None, "one end says one reason");
    }

    #[test]
    fn a_panel_that_ends_long_after_it_opened_still_has_its_record() {
        let mut creates = PersonBrowserCreates::default();
        let panel = PanelId(8);
        creates.note(panel, None);

        let ended = creates.take(panel).expect("the record of a panel that lived");

        assert_eq!(ended.link, None);
        assert_eq!(
            engine_failure_reason(&BrowserStatus::Error {
                message: "CDP connection lost: connection reset".into()
            })
            .as_deref(),
            Some("CDP connection lost: connection reset"),
            "a panel that ends with any error keeps the words of the engine"
        );
    }

    #[test]
    fn the_records_of_closed_panels_are_dropped() {
        let mut creates = PersonBrowserCreates::default();
        creates.note(PanelId(1), None);
        creates.note(PanelId(2), None);

        creates.retain(|panel| panel == PanelId(2));

        assert!(!creates.is_empty(), "the open panel stays");
        assert_eq!(creates.take(PanelId(1)), None, "the closed panel is forgotten");
        assert!(creates.take(PanelId(2)).is_some());
        assert!(creates.is_empty());
    }
}
