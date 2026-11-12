//! The one normalisation of a committed address.

use super::normalized_committed_url;

#[test]
fn blank_target_is_an_empty_committed_url() {
    assert_eq!(normalized_committed_url("about:blank"), "");
    assert_eq!(normalized_committed_url(""), "");
    assert_eq!(normalized_committed_url("https://example.com"), "https://example.com");
}
