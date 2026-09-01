//! Clipboard-preserving text paste into the focused OS client.

use crate::InjectError;

/// Paste text into the focused client without replacing the user's clipboard.
///
/// The X11 backend snapshots the current clipboard, temporarily serves `text`,
/// and restores the snapshot only after the focused client has requested the
/// staged text. A clipboard change made by another client always wins.
///
/// # Errors
///
/// Returns [`InjectError::Unsupported`] outside X11, or a clipboard/backend
/// error when the existing clipboard cannot be preserved safely or the paste
/// chord cannot be sent to the focused client.
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
