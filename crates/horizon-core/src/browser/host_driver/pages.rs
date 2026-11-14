//! The pages of one session of a browser host.
//!
//! The session of a profile on a browser host serves one page for each panel of
//! that profile. The pages are pages of one Firefox, so a person who signs in in
//! one panel is signed in in every panel of the same profile, and a private page
//! is a page of an isolated user context of the same browser: it sees none of
//! those sign-ins and leaves none behind.
//!
//! A link names its page in its greeting. A link that names a page that the
//! session has gets that page back with its typed text and its scroll place; a
//! link that names a page that the session has not gets a new page, in a new
//! window of the same Firefox. One link drives one page, and the rules of a
//! silent link and of a taken-over link stay the rules of each page.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use horizon_browser::SharedSessionGroup;

use super::session::{self, Claim, HostSession, LinkHold};
use super::{HostDriverOptions, Link, LinkEnd, LinkOutcome};

/// The most pages that one session of a browser host serves.
///
/// A page of a shared Firefox is one more window of that browser: the browser
/// keeps about 400 MB for its first window, and each page adds its content
/// process. Eight pages keep the memory of the host under about 1.5 GB, and a
/// person who needs more panels than that needs another profile on that host. A
/// panel that would pass this bound reads the number and the way out instead of
/// a browser that the host cannot carry.
pub const MAX_PAGES: usize = 8;

/// Why a link cannot drive the page that it asked for, in the words that the
/// person reads.
pub(super) struct PageRefusal {
    pub(super) code: &'static str,
    pub(super) message: String,
}

impl PageRefusal {
    fn busy(profile: &str) -> Self {
        Self {
            code: "link_busy",
            message: format!(
                "the page of this panel in the profile {profile} of this browser host is attached to another link: wait a moment and use Retry"
            ),
        }
    }

    fn starting(profile: &str) -> Self {
        Self {
            code: "page_starting",
            message: format!(
                "the page of this panel in the profile {profile} of this browser host is still starting: wait a moment and use Retry"
            ),
        }
    }

    fn full(profile: &str) -> Self {
        Self {
            code: "page_limit_reached",
            message: format!(
                "the profile {profile} of this browser host already shows {MAX_PAGES} pages, which is the most that one profile of a browser host carries: close a panel of this profile, or name another profile for the browser host"
            ),
        }
    }

    fn failed(words: String) -> Self {
        Self {
            code: "page_start",
            message: words,
        }
    }
}

/// The page of one link and the rules of that link.
pub(super) struct PageHandle {
    /// The page and everything that one link of it uses: its picture, its agent
    /// coordination and the frame that is in flight.
    pub(super) session: Mutex<HostSession>,
    pub(super) hold: LinkHold,
}

/// One page of the session, or the start of one.
enum PageEntry {
    /// A page that a link asked for, which is not ready to serve yet.
    Starting,
    Live(Arc<PageHandle>),
}

/// The pages that one session of a browser host serves.
pub(super) struct SessionPages {
    options: HostDriverOptions,
    rules: super::LinkRules,
    start: StartPage,
    /// The Firefox of this profile, made when the first page starts, so the
    /// browser and its driver are one for every page of the profile.
    group: Mutex<Option<SharedSessionGroup>>,
    pages: Mutex<HashMap<String, PageEntry>>,
}

impl SessionPages {
    pub(super) fn new(options: HostDriverOptions, rules: super::LinkRules, start: StartPage) -> Self {
        Self {
            options,
            rules,
            start,
            group: Mutex::new(None),
            pages: Mutex::new(HashMap::new()),
        }
    }

    fn pages(&self) -> std::sync::MutexGuard<'_, HashMap<String, PageEntry>> {
        self.pages.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn profile(&self) -> &str {
        self.options.profile()
    }

    /// The Firefox of this profile, which every page of the profile uses.
    fn group(&self) -> SharedSessionGroup {
        let mut group = self.group.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        group
            .get_or_insert_with(|| SharedSessionGroup::new_firefox(self.profile().to_string()))
            .clone()
    }

    /// The page that this link drives: the page that the session kept for that
    /// panel, or a new page of the profile.
    ///
    /// # Errors
    /// The page is driven by a link that still answers, the page is starting, the
    /// profile shows the most pages of a browser host, or the page did not start.
    pub(super) fn acquire(&self, page: &str, private: bool) -> Result<Arc<PageHandle>, PageRefusal> {
        let existing = {
            let pages = self.pages();
            match pages.get(page) {
                Some(PageEntry::Live(handle)) => Ok(Some(Arc::clone(handle))),
                Some(PageEntry::Starting) => Err(PageRefusal::starting(self.profile())),
                None => Ok(None),
            }
        };
        match existing {
            Ok(Some(handle)) => {
                return match handle.hold.claim() {
                    Claim::Claimed => Ok(handle),
                    Claim::Busy => Err(PageRefusal::busy(self.profile())),
                };
            }
            Err(refusal) => return Err(refusal),
            Ok(None) => {}
        }
        // Reserve the name before the browser starts, so a second link of this
        // panel reads that its page is starting rather than starting another.
        {
            let mut pages = self.pages();
            if pages.contains_key(page) {
                return Err(PageRefusal::starting(self.profile()));
            }
            if pages.len() >= MAX_PAGES {
                return Err(PageRefusal::full(self.profile()));
            }
            pages.insert(page.to_string(), PageEntry::Starting);
        }
        let started = (self.start.start)(&self.options, &self.group(), private);
        let mut pages = self.pages();
        match started {
            Ok(session) => {
                let handle = Arc::new(PageHandle {
                    session: Mutex::new(session),
                    hold: LinkHold::new(self.rules.silent, self.rules.take_over),
                });
                // The link that starts a page holds it, as the link of a page that
                // the session kept does.
                let _ = handle.hold.claim();
                pages.insert(page.to_string(), PageEntry::Live(Arc::clone(&handle)));
                Ok(handle)
            }
            Err(words) => {
                pages.remove(page);
                Err(PageRefusal::failed(words))
            }
        }
    }

    /// Greet a link, choose the page that it drives, and serve that page until
    /// the link ends.
    ///
    /// A link that names a page the session kept gets that page back; a link that
    /// names a new page gets a new page of the same browser. The words of a page
    /// that cannot be served reach the person on the link, so a panel says what to
    /// change instead of waiting.
    pub(super) fn serve_link(&self, link: &mut Link) -> ServedPage {
        let greeting = match session::greet(link) {
            Ok(greeting) => greeting,
            Err(end) => {
                session::finish(link, end);
                return ServedPage { last_page: false };
            }
        };
        let handle = match self.acquire(&greeting.page, greeting.private) {
            Ok(handle) => handle,
            Err(refusal) => {
                session::finish(
                    link,
                    LinkEnd::of(LinkOutcome::Refuse {
                        code: refusal.code.to_string(),
                        message: refusal.message,
                    }),
                );
                return ServedPage { last_page: false };
            }
        };
        let end = {
            let mut page = handle
                .session
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            session::serve_page(
                &mut page,
                link,
                self.profile(),
                self.options.engine(),
                Some(&handle.hold),
                &greeting,
            )
        };
        // The page goes before the person's link hears the end of it, so a link
        // that comes straight back opens the page of a panel that closed, and
        // never waits on the page that it just closed.
        handle.hold.release();
        let last_page = end.outcome == LinkOutcome::Close && self.page_ended(&greeting.page);
        session::finish(link, end);
        ServedPage { last_page }
    }

    /// End the page of a link that closed its panel, and answer whether the
    /// session has no page left.
    pub(super) fn page_ended(&self, page: &str) -> bool {
        let ended = self.pages().remove(page);
        if let Some(PageEntry::Live(handle)) = ended {
            handle.session.lock().unwrap_or_else(std::sync::PoisonError::into_inner).stop();
        }
        self.pages().is_empty()
    }

    /// Stop every page of this session, for the end of the session itself.
    pub(super) fn stop_all(&self) {
        let pages = std::mem::take(&mut *self.pages());
        for entry in pages.into_values() {
            if let PageEntry::Live(handle) = entry {
                handle.session.lock().unwrap_or_else(std::sync::PoisonError::into_inner).stop();
            }
        }
    }
}

/// One link served, and whether its page was the last page of the session.
pub(super) struct ServedPage {
    pub(super) last_page: bool,
}

/// How a session of a browser host starts one page.
///
/// A session of a host starts a page of the Firefox of the profile. A session of
/// a test starts a page that it owns and names no browser, so the rules of a link
/// are tested on a machine that has no Firefox.
pub(super) struct StartPage {
    start: Box<dyn Fn(&HostDriverOptions, &SharedSessionGroup, bool) -> Result<HostSession, String> + Send + Sync>,
}

impl StartPage {
    pub(super) fn new(
        start: impl Fn(&HostDriverOptions, &SharedSessionGroup, bool) -> Result<HostSession, String>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            start: Box::new(start),
        }
    }

    /// The page of a browser host: a window of the Firefox of the profile, or a
    /// page of an isolated user context of it when the link asked for one.
    pub(super) fn of_host() -> Self {
        Self::new(HostSession::start_page)
    }
}
