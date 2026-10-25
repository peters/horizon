//! Small rules about one page that the panel and its chrome share: the title
//! that a URL gives, and whether a command can navigate from inside the page.

use horizon_browser::BrowserCommand;

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

/// Whether `command` can navigate from inside the page (a click on a link, Enter
/// in a form). A pointer move and a wheel event do not navigate on their own.
#[must_use]
pub(super) fn command_may_navigate(command: &BrowserCommand) -> bool {
    matches!(
        command,
        BrowserCommand::Navigate(_)
            | BrowserCommand::Back
            | BrowserCommand::Forward
            | BrowserCommand::Reload
            | BrowserCommand::Input(
                horizon_browser::BrowserInput::MousePress { .. } | horizon_browser::BrowserInput::KeyDown { .. }
            )
    )
}

#[cfg(test)]
mod tests {
    use super::panel_title_for_url;

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
