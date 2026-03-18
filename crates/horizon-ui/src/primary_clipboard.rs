/// Copy `text` into the system primary selection buffer.
///
/// On Linux, uses `arboard` with `LinuxClipboardKind::Primary`.
/// No-op on other platforms where primary selection does not exist.
pub fn copy_to_primary(text: &str) {
    #[cfg(target_os = "linux")]
    {
        use arboard::{Clipboard, SetExtLinux as _};

        let Ok(mut clipboard) = Clipboard::new() else {
            tracing::debug!("failed to open clipboard for primary selection copy");
            return;
        };
        if let Err(error) = clipboard
            .set()
            .clipboard(arboard::LinuxClipboardKind::Primary)
            .text(text.to_owned())
        {
            tracing::debug!("failed to copy to primary selection: {error}");
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = text;
    }
}

/// Read text from the system primary selection buffer.
///
/// Returns `None` on non-Linux platforms or when the buffer is empty /
/// inaccessible.
pub fn paste_from_primary() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        use arboard::{Clipboard, GetExtLinux as _};

        let mut clipboard = Clipboard::new().ok()?;
        clipboard
            .get()
            .clipboard(arboard::LinuxClipboardKind::Primary)
            .text()
            .ok()
            .filter(|text| !text.is_empty())
    }

    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}
