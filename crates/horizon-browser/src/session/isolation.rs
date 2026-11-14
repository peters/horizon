//! What a session proves about the page that it drives: whether the page can
//! reach the cookies and the storage of a browser profile, and what keeps it
//! apart.
//!
//! A panel that says `Private` says that the page is signed in nowhere and
//! leaves nothing of itself in a profile. That is a fact of the running session
//! and never a copy of the request that started it: a session publishes what it
//! established here, and a page that no fact covers is [`PageIsolation::Shared`],
//! which claims nothing.

use std::sync::{Arc, Mutex};

/// The word that a session uses when it cannot make the private page that a
/// panel asked for. A caller answers it as the code of a failed create.
pub const PRIVATE_PAGE_UNAVAILABLE: &str = "private_page_unavailable";

/// Whether the page of a session keeps nothing of a browser profile, and what
/// proves it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PageIsolation {
    /// The page reads the cookies and the storage of the profile that it named.
    /// This is also the answer of a page that no session has reported yet: a
    /// page that nothing proves isolated is never shown as private.
    #[default]
    Shared,
    /// A page of an isolated browser context of a browser that this session
    /// started: `Target.createBrowserContext` keeps its cookies and its storage
    /// apart from the profile, and disposing the context removes them.
    IsolatedContext,
    /// The page of a browser that this session started in a folder that this
    /// panel alone holds, which no other page joins and which its close removes.
    PrivateFolder,
}

impl PageIsolation {
    /// Whether this page is in no profile: a context of its own or a folder of
    /// its own. Only this answer allows a panel to call the page private.
    #[must_use]
    pub const fn keeps_nothing(self) -> bool {
        matches!(self, Self::IsolatedContext | Self::PrivateFolder)
    }

    /// The words of the page-isolation report of a panel, for a document that
    /// reads as `private` or as a profile name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::IsolatedContext => "isolated_context",
            Self::PrivateFolder => "private_folder",
        }
    }
}

/// The fact that a panel reads: what the session of its page established.
///
/// The session publishes once, as soon as it knows, and the panel reads the
/// answer at any time. A panel starts a new report for a new session, so a fact
/// of the link before never describes the page of the link now.
#[derive(Clone, Debug, Default)]
pub struct PageIsolationReport(Arc<Mutex<Option<PageIsolation>>>);

impl PageIsolationReport {
    /// Publish what this session established about its page.
    pub fn publish(&self, isolation: PageIsolation) {
        *self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(isolation);
    }

    /// What this session established, and `None` while it says nothing.
    #[must_use]
    pub fn snapshot(&self) -> Option<PageIsolation> {
        *self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// What the page keeps, with the shared answer for a session that reported
    /// nothing: an unproven page is never called private.
    #[must_use]
    pub fn isolation(&self) -> PageIsolation {
        self.snapshot().unwrap_or_default()
    }
}

/// The refusal of a session that was asked for a private page and cannot prove
/// that its page is one. The words say what failed and that no page of the
/// shared profile was opened in its place.
#[must_use]
pub fn private_page_unavailable(detail: &str) -> String {
    format!(
        "{PRIVATE_PAGE_UNAVAILABLE}: {detail}, so this panel opened no page in the shared profile. \
         A page that keeps nothing needs a browser of a browser host that answers its own browser context"
    )
}

/// The reason inside a refusal of [`private_page_unavailable`], for a panel that
/// reads the words of a stopped session.
#[must_use]
pub fn private_page_unavailable_detail(message: &str) -> Option<&str> {
    message
        .strip_prefix(PRIVATE_PAGE_UNAVAILABLE)
        .and_then(|rest| rest.strip_prefix(": "))
        .map(|detail| detail.split(',').next().unwrap_or(detail))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_that_nothing_proves_isolated_is_never_private() {
        let report = PageIsolationReport::default();
        assert_eq!(report.snapshot(), None, "a session that started says nothing yet");
        assert_eq!(report.isolation(), PageIsolation::Shared);
        assert!(!report.isolation().keeps_nothing());

        report.publish(PageIsolation::IsolatedContext);
        assert_eq!(report.snapshot(), Some(PageIsolation::IsolatedContext));
        assert!(report.isolation().keeps_nothing());

        let shared = PageIsolationReport::default();
        shared.publish(PageIsolation::Shared);
        assert_eq!(shared.snapshot(), Some(PageIsolation::Shared));
        assert!(!shared.isolation().keeps_nothing(), "a shared page is no private page");
    }

    #[test]
    fn a_private_page_folder_is_a_page_that_keeps_nothing() {
        assert!(PageIsolation::PrivateFolder.keeps_nothing());
        assert_eq!(PageIsolation::PrivateFolder.as_str(), "private_folder");
        assert_eq!(PageIsolation::IsolatedContext.as_str(), "isolated_context");
        assert_eq!(PageIsolation::Shared.as_str(), "shared");
    }

    #[test]
    fn the_refusal_names_what_failed_and_that_no_shared_page_was_opened() {
        let message = private_page_unavailable("the browser refused a browser context of its own (error 3)");
        assert!(
            message.starts_with("private_page_unavailable: the browser refused"),
            "{message}"
        );
        assert!(message.contains("opened no page in the shared profile"), "{message}");
        assert_eq!(
            private_page_unavailable_detail(&message),
            Some("the browser refused a browser context of its own (error 3)")
        );
        assert_eq!(private_page_unavailable_detail("backend_start_failed"), None);
    }
}
