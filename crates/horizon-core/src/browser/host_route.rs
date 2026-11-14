//! Which machine and which profile serve a new browser panel.
//!
//! One rule decides every new browser panel: the panel that an agent asks for
//! with `browser_create`, the panel that a person makes from the picker, from a
//! shortcut or from a link of a card, and the item `Private panel` of a panel
//! menu. The rule reads three things and nothing else: the entry `browser_host`
//! of the settings, the `profile` field of the create, and the engine that the
//! create named. The panel, the answer of the create and the note beside it
//! therefore cannot disagree.
//!
//! Without a host the rule answers the browser of this computer, as it did
//! before the entry existed.

use horizon_browser::ProfileChoice;

use crate::browser::BackendKind;
use crate::config::BrowserHostConfig;

/// The machine that serves the page of a new browser panel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowserPlace {
    /// The browser host of the settings serves the page, in the engine of that
    /// host. `private` says that the page keeps nothing of the profile of that
    /// machine: it is a page of an isolated context of the same browser.
    Host { private: bool },
    /// This computer serves the page, in `backend`.
    ThisComputer { backend: BackendKind },
}

impl BrowserPlace {
    /// Whether the browser of the browser host of the settings serves this page.
    #[must_use]
    pub const fn is_host(self) -> bool {
        matches!(self, Self::Host { .. })
    }
}

/// What one create asks for, before any rule reads it.
///
/// It is the two fields of a create that decide the machine, the engine and the
/// profile of a new browser panel, as its caller wrote them. A panel that a
/// person makes from the picker writes no field, so its preset is the configured
/// engine of this computer and not a create field.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BrowserPanelRequest {
    /// The `profile` field as the caller wrote it, and `None` when the caller
    /// wrote no field. A create with no field is a page of the browser host of
    /// the settings, in the profile of that host.
    pub profile: Option<String>,
    /// The `backend` field as the caller wrote it, and `None` when the caller
    /// named no engine.
    pub backend: Option<BackendKind>,
}

/// What one create asks for, before any rule reads it, and what the rule answers
/// for it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewBrowserPanel {
    /// The machine and the engine that serve the page.
    pub place: BrowserPlace,
    /// The profile that the page reads: the shared profile of this computer that
    /// it joins, or `None` for a private page. A page of the browser host takes
    /// the profile of the entry, so this field says nothing about it.
    pub profile: Option<String>,
    /// Whether the page keeps nothing of a profile.
    pub private: bool,
    /// The words that the answer of the create carries when the page does not
    /// run where the settings name a host, so an agent and a person read where
    /// it runs. `None` for a page that runs where the settings name it.
    pub note: Option<String>,
}

impl NewBrowserPanel {
    /// The profile that the panel reports for this page: the name of the shared
    /// profile that it reads, or `private`.
    #[must_use]
    pub fn reported_profile(&self) -> Option<&str> {
        if self.private {
            return Some(horizon_browser::PRIVATE_PROFILE);
        }
        self.profile.as_deref()
    }
}

/// The rule: where the browser panel of one create runs.
///
/// - `profile` is the `profile` field of the create as its caller wrote it, and
///   `None` when the caller wrote no field or a blank one.
/// - `named_backend` is the `backend` field of the create as its caller wrote
///   it. A panel that a person makes names no engine, so the engine of the
///   preset is the configured engine and not a create field.
/// - `configured_backend` is the engine of the `browser` settings, which serves
///   a page of this computer when the create names no engine.
///
/// With a browser host in the settings:
///
/// - a create with no `profile` is a page of the profile of that host, in the
///   engine of that host;
/// - a create with no `profile` that names another engine is a panel of this
///   computer in that engine, in the shared profile `default` of this computer,
///   so a person can check a page in another engine;
/// - `profile: "private"` is a private page of that host, in the engine of that
///   host, or a private panel of this computer when the create names another
///   engine;
/// - a create that names a profile keeps a profile of this computer. The shared
///   profile `default` is such a name: a create that writes it asks for the
///   profile of this computer, and a create that writes no field asks for the
///   profile of the host.
///
/// Without a host, and for a create that names a profile, the answer is a panel
/// of this computer in the profile and the engine that the create asked for.
///
/// # Errors
/// Returns the words that the profile-name rule gives for a name that it
/// refuses, so an agent learns what to change.
pub fn new_browser_panel(
    profile: Option<&str>,
    named_backend: Option<BackendKind>,
    configured_backend: BackendKind,
    host: &BrowserHostConfig,
) -> Result<NewBrowserPanel, String> {
    let named_profile = profile.map(str::trim).filter(|name| !name.is_empty());
    let choice = ProfileChoice::parse(named_profile)?;
    let private = matches!(choice, ProfileChoice::Private);
    // The host serves the page when it serves that engine, and when the create
    // asks for the profile of that machine: no field, or the reserved word of a
    // page that keeps nothing.
    let host_serves = host.names_host()
        && (named_profile.is_none() || private)
        && named_backend.is_none_or(|backend| host.backend_refusal(backend).is_none());
    if host_serves {
        return Ok(NewBrowserPanel {
            place: BrowserPlace::Host { private },
            profile: None,
            private,
            note: None,
        });
    }
    let backend = named_backend.unwrap_or(configured_backend);
    Ok(NewBrowserPanel {
        place: BrowserPlace::ThisComputer { backend },
        profile: choice.shared_name().map(str::to_string),
        private,
        note: note_for_this_computer(host, backend, &choice),
    })
}

/// Fill the place of a browser panel of `opts` from the rule of
/// [`new_browser_panel`], so the panel that opens, the profile that it saves and
/// the answer of the create read one answer.
///
/// The app hands the entry of the settings that this run holds, so a panel that
/// is created after a confirmed change of the entry uses that host at once.
///
/// # Errors
/// Returns the words of the rule for a profile name that it refuses.
pub fn place_browser_panel(
    opts: &mut crate::PanelOptions,
    host: &BrowserHostConfig,
    configured: BackendKind,
) -> Result<(), String> {
    let request = opts.browser_request.clone().unwrap_or_default();
    let panel = new_browser_panel(request.profile.as_deref(), request.backend, configured, host)?;
    opts.browser_place = Some(panel.place);
    opts.browser_session_id.clone_from(&panel.profile);
    opts.browser_private = panel.private;
    opts.browser_host_entry = Some(host.clone());
    Ok(())
}

/// The words that the answer of a create carries when a browser host is in the
/// settings and the page does not run on it.
///
/// A person reads the note in the sidebar, and an agent reads it in the answer,
/// so both learn why the page is here: a page of another engine, a private page
/// of this computer, or a shared profile of this computer that the create named.
fn note_for_this_computer(host: &BrowserHostConfig, backend: BackendKind, choice: &ProfileChoice) -> Option<String> {
    if !host.names_host() {
        return None;
    }
    let engine = backend.display_name();
    let host_engine = host.engine().backend().display_name();
    let other_engine = host.backend_refusal(backend).is_some();
    Some(match (choice, other_engine) {
        (ProfileChoice::Private, _) => format!(
            "This private page runs in the {engine} of this computer, not on the browser host, because the create named \
             another engine than the {host_engine} of that host."
        ),
        (ProfileChoice::Shared(name), true) => format!(
            "This page runs in the {engine} of this computer in the shared profile `{name}`, not on the browser host, \
             because the create named another engine than the {host_engine} of that host."
        ),
        (ProfileChoice::Shared(name), false) => format!(
            "This page runs in the shared profile `{name}` of this computer, not on the browser host, which serves the \
             profile of its own engine."
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(engine: crate::config::BrowserHostEngine, profile: Option<&str>) -> BrowserHostConfig {
        BrowserHostConfig {
            ssh_target: Some("browser-vm".to_string()),
            profile: profile.map(str::to_string),
            engine,
            ..BrowserHostConfig::default()
        }
    }

    fn chromium_host() -> BrowserHostConfig {
        host(crate::config::BrowserHostEngine::Chromium, Some("live42"))
    }

    fn firefox_host() -> BrowserHostConfig {
        host(crate::config::BrowserHostEngine::Firefox, Some("live42"))
    }

    fn route(profile: Option<&str>, named: Option<BackendKind>, host: &BrowserHostConfig) -> NewBrowserPanel {
        new_browser_panel(profile, named, BackendKind::ChromiumCdp, host).expect("a create the rule accepts")
    }

    #[test]
    fn a_create_with_no_profile_is_a_page_of_the_host_in_the_engine_of_the_host() {
        for (enter, engine) in [
            (chromium_host(), BackendKind::ChromiumCdp),
            (firefox_host(), BackendKind::FirefoxBidi),
        ] {
            let panel = route(None, None, &enter);
            assert_eq!(
                panel.place,
                BrowserPlace::Host { private: false },
                "a create with no profile is a page of the host"
            );
            assert!(!panel.private);
            assert!(panel.note.is_none(), "a page of the host needs no note");
            assert_eq!(
                engine.display_name(),
                enter.engine().backend().display_name(),
                "the host serves the engine of its entry"
            );
        }
    }

    #[test]
    fn a_create_with_no_profile_that_names_another_engine_is_a_panel_of_this_computer() {
        let panel = route(None, Some(BackendKind::FirefoxBidi), &chromium_host());
        assert_eq!(
            panel.place,
            BrowserPlace::ThisComputer {
                backend: BackendKind::FirefoxBidi
            },
            "the create named Firefox, and a Chromium host serves no Firefox page"
        );
        assert_eq!(
            panel.profile.as_deref(),
            Some(horizon_browser::DEFAULT_PROFILE),
            "the panel of this computer joins the shared default profile"
        );
        let note = panel.note.expect("the answer says that the page is not on the host");
        assert!(note.contains("this computer"), "{note}");
        assert!(note.contains("browser host"), "{note}");
        assert!(note.contains("Firefox"), "{note}");
    }

    #[test]
    fn a_private_create_is_a_private_page_of_the_host_or_of_this_computer() {
        let on_host = route(Some(horizon_browser::PRIVATE_PROFILE), None, &firefox_host());
        assert_eq!(on_host.place, BrowserPlace::Host { private: true });
        assert!(on_host.private);
        assert_eq!(
            on_host.reported_profile(),
            Some(horizon_browser::PRIVATE_PROFILE),
            "a private page of the host reports the reserved word"
        );

        let here = route(
            Some(horizon_browser::PRIVATE_PROFILE),
            Some(BackendKind::ChromiumCdp),
            &firefox_host(),
        );
        assert_eq!(
            here.place,
            BrowserPlace::ThisComputer {
                backend: BackendKind::ChromiumCdp
            },
            "a private create of another engine keeps a private page of this computer"
        );
        assert!(here.private);
        assert!(here.profile.is_none(), "a private page joins no shared profile");
        assert!(
            here.note
                .expect("the answer names the machine")
                .contains("this computer")
        );
    }

    #[test]
    fn a_create_that_names_a_profile_keeps_a_profile_of_this_computer() {
        let panel = route(Some("work"), None, &chromium_host());
        assert_eq!(
            panel.place,
            BrowserPlace::ThisComputer {
                backend: BackendKind::ChromiumCdp
            },
            "the host serves its own profile only"
        );
        assert_eq!(panel.profile.as_deref(), Some("work"));
        assert!(!panel.private);
        let note = panel.note.expect("the answer says that the page is not on the host");
        assert!(note.contains("work"), "{note}");
        assert!(note.contains("this computer"), "{note}");
    }

    #[test]
    fn a_create_that_names_the_shared_default_asks_for_the_profile_of_this_computer() {
        let panel = route(Some(horizon_browser::DEFAULT_PROFILE), None, &firefox_host());
        assert_eq!(
            panel.place,
            BrowserPlace::ThisComputer {
                backend: BackendKind::ChromiumCdp
            },
            "a create that writes the name default asks for a page of this computer"
        );
        assert_eq!(
            panel.reported_profile(),
            Some(horizon_browser::DEFAULT_PROFILE),
            "the page joins the shared default of this computer"
        );
        let note = panel.note.expect("the answer says that the page is not on the host");
        assert!(note.contains(horizon_browser::DEFAULT_PROFILE), "{note}");
        assert!(note.contains("this computer"), "{note}");
        let blank = route(Some("  "), None, &firefox_host());
        assert_eq!(
            blank.place,
            BrowserPlace::Host { private: false },
            "a blank field names no profile, so the host serves the page"
        );
    }

    #[test]
    fn without_a_browser_host_every_create_is_a_page_of_this_computer() {
        for enter in [
            BrowserHostConfig::default(),
            BrowserHostConfig {
                profile: Some("live42".to_string()),
                ..BrowserHostConfig::default()
            },
        ] {
            let panel = route(None, None, &enter);
            assert_eq!(
                panel.place,
                BrowserPlace::ThisComputer {
                    backend: BackendKind::ChromiumCdp
                }
            );
            assert_eq!(
                panel.profile.as_deref(),
                Some(horizon_browser::DEFAULT_PROFILE),
                "a create that names no profile joins the shared default, as before the host entry"
            );
            assert!(panel.note.is_none(), "no host names no note");
        }

        let named = route(None, Some(BackendKind::FirefoxBidi), &BrowserHostConfig::default());
        assert_eq!(
            named.place,
            BrowserPlace::ThisComputer {
                backend: BackendKind::FirefoxBidi
            },
            "a created engine decides the panel of this computer"
        );
    }

    #[test]
    fn a_name_that_the_profile_rule_refuses_is_the_words_of_that_rule() {
        let error = new_browser_panel(Some("My profile"), None, BackendKind::ChromiumCdp, &chromium_host())
            .expect_err("the profile rule refuses a space");
        assert!(error.contains("a-z, 0-9"), "{error}");
    }
}
