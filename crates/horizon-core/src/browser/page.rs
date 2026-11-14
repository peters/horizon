//! Small rules about one page that the panel and its chrome share: the title
//! that a URL gives, and whether a command can navigate from inside the page.

use horizon_browser::BrowserCommand;

use super::BrowserPanelState;

/// Short human-facing title for a URL (hostname, or a non-empty fallback).
#[must_use]
pub fn panel_title_for_url(url: &str) -> String {
    let input = url.trim();
    if input.is_empty() {
        return "Browser".to_string();
    }
    let Some((_, scheme_specific)) = input.split_once("://") else {
        return input.to_string();
    };
    let authority = scheme_specific.split(['/', '?', '#']).next().unwrap_or_default();
    let host_and_port = authority.rsplit('@').next().unwrap_or_default();
    let host = if let Some(bracketed) = host_and_port.strip_prefix('[') {
        bracketed.split_once(']').map_or("", |(host, _)| host)
    } else {
        host_and_port.split(':').next().unwrap_or_default()
    };
    if host.is_empty() {
        input.to_string()
    } else {
        host.to_string()
    }
}

/// Whether `command` is a navigation that the person asked for.
///
/// A click or a key may navigate from inside the page, and it counts as no
/// navigation here: a person who clicks a new panel before its first page
/// commits sent a create that reported `superseded` for the page that it had
/// just loaded. A create reads the address that the page commits instead.
#[must_use]
pub(super) fn command_is_a_navigation(command: &BrowserCommand) -> bool {
    matches!(
        command,
        BrowserCommand::Navigate(_) | BrowserCommand::Back | BrowserCommand::Forward | BrowserCommand::Reload
    )
}

impl BrowserPanelState {
    /// Queue a command only when the driver session currently exists.
    #[must_use]
    pub fn try_send(&self, command: BrowserCommand) -> bool {
        // A command that is a navigation counts as the navigation of the person,
        // and it may take a pending startup navigation over. A click or a key
        // does not: it may navigate, and a person who reads a new panel clicks it
        // before its first page commits, which made a create answer `superseded`
        // for the page that it had just loaded.
        let navigation = command_is_a_navigation(&command);
        let accepted = self.deliver(command);
        if accepted && navigation {
            self.user_navigations.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        accepted
    }

    /// Send the first navigation of a panel, which is the address of its create
    /// and not a navigation of the person, so it never takes that create over.
    pub(super) fn send_startup_navigation(&self, url: String) -> bool {
        self.deliver(BrowserCommand::Navigate(url))
    }

    /// Hand one command to the driver of this panel, whichever lane it runs on.
    fn deliver(&self, command: BrowserCommand) -> bool {
        if self.is_host_driver() {
            self.send_host_driver_command(command)
        } else {
            self.session.as_ref().is_some_and(|session| session.send(command))
        }
    }

    /// The navigations of the person on this panel so far: a typed address, back,
    /// forward and reload. A create of the panel reads this count, so a click on
    /// the new panel is no navigation of the person.
    #[must_use]
    pub fn user_navigation_count(&self) -> u32 {
        self.user_navigations.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::{BrowserCommand, command_is_a_navigation, panel_title_for_url};

    #[test]
    fn only_a_navigation_command_counts_as_a_navigation_of_the_person() {
        for command in [
            BrowserCommand::Navigate("https://example.test/".to_string()),
            BrowserCommand::Back,
            BrowserCommand::Forward,
            BrowserCommand::Reload,
        ] {
            assert!(command_is_a_navigation(&command), "{command:?}");
        }
        for command in [
            BrowserCommand::Input(horizon_browser::BrowserInput::MousePress {
                x: 1.0,
                y: 2.0,
                button: horizon_browser::BrowserButton::Left,
                click_count: 1,
                buttons: 1,
                modifiers: horizon_browser::BrowserModifiers::none(),
            }),
            BrowserCommand::Input(horizon_browser::BrowserInput::KeyDown {
                physical_key: None,
                key: horizon_browser::BrowserKey::Enter,
                text: None,
                modifiers: horizon_browser::BrowserModifiers::none(),
                repeat: false,
                edit_command: None,
            }),
            BrowserCommand::Stop,
            BrowserCommand::SetViewport {
                width: 800,
                height: 600,
            },
        ] {
            assert!(!command_is_a_navigation(&command), "{command:?}");
        }
    }

    #[test]
    fn panel_titles_extract_dns_and_bracketed_ipv6_hosts() {
        assert_eq!(panel_title_for_url("https://example.com:8443/path"), "example.com");
        assert_eq!(panel_title_for_url("http://[::1]:3000/path"), "::1");
        assert_eq!(panel_title_for_url("https://user:secret@example.net/a"), "example.net");
    }

    #[test]
    fn panel_titles_keep_non_empty_fallbacks_for_hostless_urls() {
        assert_eq!(panel_title_for_url("file:///tmp/page.html"), "file:///tmp/page.html");
        assert_eq!(panel_title_for_url("about:blank"), "about:blank");
        assert_eq!(panel_title_for_url("  "), "Browser");
    }
}
