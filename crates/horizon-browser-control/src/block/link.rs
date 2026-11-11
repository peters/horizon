//! The one address a card may carry, and the host that the person sees.
//!
//! An agent writes the URL of a `link` component, so Horizon reads it before a
//! card draws it: only a page that a browser serves over `http` or `https`, only
//! with a host, and never with credentials in front of the host. Nothing is
//! fetched here; the address reaches a browser panel only when the person
//! presses the link.
//!
//! The host is what the card shows under the label, because the label is the
//! agent's text and can name another site. It is normalized as the browser
//! resolves it, so a host that a script of lookalike letters spells is drawn
//! with its own address and not as the site it imitates.

use url::Url;

/// The schemes that a link may name.
const SCHEMES: [&str; 2] = ["http", "https"];

/// The address `url` as a browser resolves it, when Horizon may open it.
///
/// # Errors
/// `None` for anything that is not an absolute `http` or `https` URL with a
/// host, and for an address with credentials, whitespace or a control
/// character.
#[must_use]
pub fn checked_link(url: &str) -> Option<Url> {
    let plain = url
        .chars()
        .all(|character| !character.is_whitespace() && !character.is_control());
    if !plain {
        return None;
    }
    let parsed = Url::parse(url).ok()?;
    if !SCHEMES.contains(&parsed.scheme()) || parsed.host_str().is_none() {
        return None;
    }
    // `https://@host/` names a user without a name; `https://:pass@host/` names
    // a password without a user. Both hide part of the address from the person.
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return None;
    }
    Some(parsed)
}

/// The host of `url`, with its port when the URL states a port that the scheme
/// does not already imply, and `None` when Horizon may not open `url`.
///
/// This is the whole destination that a card shows under the label of a link.
#[must_use]
pub fn link_host(url: &str) -> Option<String> {
    let parsed = checked_link(url)?;
    let host = parsed.host_str()?;
    Some(match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::{checked_link, link_host};

    #[test]
    fn a_plain_page_address_is_one_that_horizon_opens() {
        assert!(checked_link("https://example.test/session/1").is_some());
        assert!(checked_link("http://example.test:8080/").is_some());
        assert!(checked_link("https://[::1]:8443/page").is_some());
    }

    #[test]
    fn only_the_two_page_schemes_are_opened() {
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,<b>x</b>",
            "ftp://example.test/",
            "ssh://example.test/",
            "mailto:agent@example.test",
            "example.test/session",
            "",
        ] {
            assert!(checked_link(url).is_none(), "{url} must be refused");
        }
        assert!(checked_link("https://").is_none(), "a URL without a host");
    }

    #[test]
    fn credentials_are_refused() {
        assert!(checked_link("https://user:pass@example.test/").is_none());
        assert!(checked_link("https://user@example.test/").is_none());
        assert!(checked_link("https://:pass@example.test/").is_none());
    }

    #[test]
    fn spaces_and_controls_are_refused() {
        for url in [
            "https://example.test/a b",
            " https://example.test/",
            "https://example.test/\n",
            "https://example.test/\u{7}",
            "https://exa\u{2028}mple.test/",
        ] {
            assert!(checked_link(url).is_none(), "{url:?} must be refused");
        }
    }

    #[test]
    fn the_host_omits_a_port_that_the_scheme_implies() {
        assert_eq!(link_host("https://example.test/a/b").as_deref(), Some("example.test"));
        assert_eq!(link_host("https://example.test:443/").as_deref(), Some("example.test"));
        assert_eq!(
            link_host("https://example.test:8443/deep/path?q=1#top").as_deref(),
            Some("example.test:8443")
        );
        assert_eq!(link_host("http://example.test:80/").as_deref(), Some("example.test"));
        assert_eq!(link_host("https://[::1]:8443/").as_deref(), Some("[::1]:8443"));
    }

    #[test]
    fn the_host_is_the_address_and_not_the_letters_of_a_lookalike() {
        // An `a` of the Cyrillic block spells the same as the Latin one on screen.
        assert_eq!(
            link_host("https://\u{430}pple.test/").as_deref(),
            Some("xn--pple-43d.test")
        );
        assert_eq!(
            link_host("https://m\u{fc}nchen.example/").as_deref(),
            Some("xn--mnchen-3ya.example")
        );
        assert_eq!(link_host("https://EXAMPLE.test/").as_deref(), Some("example.test"));
    }

    #[test]
    fn a_refused_address_has_no_host_to_show() {
        assert_eq!(link_host("https://user:pass@example.test/"), None);
        assert_eq!(link_host("javascript:alert(1)"), None);
    }
}
