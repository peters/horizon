//! Clipboard-preserving text paste into the focused OS client.

use crate::InjectError;

/// Paste text into the focused client without replacing the user's clipboard.
///
/// X11 restores every advertised format after the target reads `text`; a newer
/// clipboard change always wins.
///
/// # Errors
/// Unsupported outside X11, or if preservation or paste-chord delivery fails.
pub fn paste_text_preserving_clipboard(text: &str) -> Result<(), InjectError> {
    platform::paste_text_preserving_clipboard(text)
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod platform;

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
mod platform {
    use crate::InjectError;

    pub(super) fn paste_text_preserving_clipboard(_text: &str) -> Result<(), InjectError> {
        Err(InjectError::Unsupported)
    }
}
