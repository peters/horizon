use alacritty_terminal::term::ClipboardType;

/// Destination named by an OSC 52 copy request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardTarget {
    /// The system clipboard (`c`).
    Clipboard,
    /// The primary selection (`p` / `s`); platforms without one ignore it.
    Selection,
}

/// A copy request a program running in the terminal made through OSC 52.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardWrite {
    pub target: ClipboardTarget,
    pub text: String,
}

/// Latest unforwarded OSC 52 copy per destination.
///
/// A newer copy supersedes an unforwarded older one, so a program that copies
/// repeatedly between two frames cannot queue unbounded text.
#[derive(Default)]
pub(super) struct PendingClipboard {
    clipboard: Option<String>,
    selection: Option<String>,
}

impl PendingClipboard {
    pub(super) fn store(&mut self, clipboard: ClipboardType, text: String) {
        match clipboard {
            ClipboardType::Clipboard => self.clipboard = Some(text),
            ClipboardType::Selection => self.selection = Some(text),
        }
    }

    pub(super) fn take(&mut self) -> Vec<ClipboardWrite> {
        [
            (ClipboardTarget::Clipboard, self.clipboard.take()),
            (ClipboardTarget::Selection, self.selection.take()),
        ]
        .into_iter()
        .filter_map(|(target, text)| text.map(|text| ClipboardWrite { target, text }))
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_copy_replaces_an_unforwarded_one_per_target() {
        let mut pending = PendingClipboard::default();
        pending.store(ClipboardType::Clipboard, "first".to_owned());
        pending.store(ClipboardType::Selection, "picked".to_owned());
        pending.store(ClipboardType::Clipboard, "second".to_owned());

        assert_eq!(
            pending.take(),
            vec![
                ClipboardWrite {
                    target: ClipboardTarget::Clipboard,
                    text: "second".to_owned()
                },
                ClipboardWrite {
                    target: ClipboardTarget::Selection,
                    text: "picked".to_owned()
                },
            ]
        );
        assert!(pending.take().is_empty());
    }
}
