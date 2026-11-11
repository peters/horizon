//! The QR code of a join link: the modules that a window paints and the text that a
//! terminal prints.
//!
//! One link is shown as one QR code of error correction level M, which holds the
//! text `horizon://<host>:<port>/join#<CODE>` and nothing else. The encoder is
//! `qrcodegen`, which has no dependency of its own, so no second crate of the tree
//! arrives with it.
//!
//! The text form is for the mode that makes the first invitation of a server on
//! the server computer itself: one module is two characters wide, so a module is
//! square in a terminal cell, and two module rows share one character by their
//! halves. The quiet zone is four modules of light on every side, which the
//! standard asks for, and the code is dark on light.
//!
//! The window does not use the text: it paints the modules as rectangles, and
//! [`Qr::is_dark`] answers one module.

use std::fmt;

use qrcodegen::{QrCode, QrCodeEcc};

/// The quiet zone of a QR code, in modules, as the standard asks for it.
pub const QUIET_ZONE: usize = 4;

/// Why a text cannot be one QR code.
#[derive(Debug, thiserror::Error)]
pub enum QrError {
    /// The text is too long for the largest code of this error correction level.
    #[error("the text is too long for one QR code: {0}")]
    TooLong(String),
}

/// One QR code of one text.
#[derive(Clone, PartialEq, Eq)]
pub struct Qr {
    code: QrCode,
}

impl fmt::Debug for Qr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Qr")
            .field("size", &self.size())
            .finish_non_exhaustive()
    }
}

impl Qr {
    /// The code of `text`, at error correction level M.
    ///
    /// # Errors
    /// Refuses a text that is longer than the largest code of that level.
    pub fn encode(text: &str) -> Result<Self, QrError> {
        QrCode::encode_text(text, QrCodeEcc::Medium)
            .map(|code| Self { code })
            .map_err(|error| QrError::TooLong(error.to_string()))
    }

    /// The number of modules of one side, without the quiet zone.
    #[must_use]
    pub fn size(&self) -> usize {
        usize::try_from(self.code.size()).unwrap_or(0)
    }

    /// Whether the module at `x` and `y` is dark. A coordinate outside the code is
    /// light, which is the quiet zone.
    #[must_use]
    pub fn is_dark(&self, x: usize, y: usize) -> bool {
        let (Ok(x), Ok(y)) = (i32::try_from(x), i32::try_from(y)) else {
            return false;
        };
        (x, y) < (self.code.size(), self.code.size()) && x >= 0 && y >= 0 && self.code.get_module(x, y)
    }

    /// The code as text: two characters per module, two module rows per text row,
    /// with a quiet zone of [`QUIET_ZONE`] modules and dark on light.
    #[must_use]
    pub fn text(&self) -> String {
        const LIGHT: &str = "  ";
        const TOP: &str = "\u{2580} ";
        const BOTTOM: &str = "\u{2584} ";
        const FULL: &str = "\u{2588} ";
        let modules = self.size();
        let side = modules + 2 * QUIET_ZONE;
        let mut text = String::with_capacity((side * 2 + 1) * side.div_ceil(2));
        let mut row = 0;
        while row < side {
            for column in 0..side {
                let module = |x: usize, y: usize| {
                    let (Some(x), Some(y)) = (x.checked_sub(QUIET_ZONE), y.checked_sub(QUIET_ZONE)) else {
                        return false;
                    };
                    self.is_dark(x, y)
                };
                let top = module(column, row);
                let bottom = module(column, row + 1);
                text.push_str(match (top, bottom) {
                    (false, false) => LIGHT,
                    (true, false) => TOP,
                    (false, true) => BOTTOM,
                    (true, true) => FULL,
                });
            }
            while text.ends_with(' ') {
                text.pop();
            }
            text.push('\n');
            row += 2;
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text of one known link, which a person reads on the server computer.
    /// The expectation is checked in: a change of the encoder, of the level or of
    /// the text form fails this test and has to be looked at.
    #[test]
    fn the_text_of_a_known_link_is_the_checked_in_text() {
        let qr = Qr::encode("horizon://127.0.0.1:7000/join#ABCD-EFGH-JKMN").expect("a code");
        assert_eq!(qr.size(), 33, "a version 4 code of level M holds this link");
        assert_eq!(qr.text(), KNOWN_TEXT);
    }

    #[test]
    fn a_code_holds_its_quiet_zone_and_dark_on_light() {
        let qr = Qr::encode("horizon://server:7000/join#ABCD-EFGH-JKMN").expect("a code");
        assert!(qr.is_dark(0, 0), "the first module of a QR code is dark");
        assert!(!qr.is_dark(qr.size(), 0), "outside the code is the quiet zone");
        let text = qr.text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), (qr.size() + 2 * QUIET_ZONE).div_ceil(2));
        for line in &lines {
            let characters = line.chars().count();
            assert!(characters <= 2 * (qr.size() + 2 * QUIET_ZONE), "{line:?}");
            assert!(
                line.chars()
                    .all(|character| matches!(character, ' ' | '\u{2580}' | '\u{2584}' | '\u{2588}')),
                "a text row holds halves of modules only: {line:?}"
            );
            assert!(!line.ends_with(' '), "the quiet zone of the right is one gap: {line:?}");
        }
        // The quiet zone is light, so the first two rows of the text hold none of
        // the dark characters of the code.
        for line in lines.iter().take(QUIET_ZONE / 2) {
            assert!(line.trim().is_empty(), "{line:?}");
        }
        assert!(
            Qr::encode(&"a".repeat(4000)).is_err(),
            "a text that is too long is refused"
        );
    }

    const KNOWN_TEXT: &str = "\n\n        █ ▀ ▀ ▀ ▀ ▀ █   ▄ █ █ ▄ █   █ ▄   ▀ ▄ ▀   █ ▄ █ █   █ ▀ ▀ ▀ ▀ ▀ █\n        █   █ █ █   █   ▀ ▄ ▀ ▄   ▄ █ ▄ ▄ ▀     ▀ ▄ █ ▀ █   █   █ █ █   █\n        █   ▀ ▀ ▀   █       ▄ ▀ ▄ ▀   ▄ ▄ ▀   ▄ ▀ █   ▀ █   █   ▀ ▀ ▀   █\n        ▀ ▀ ▀ ▀ ▀ ▀ ▀   ▀ ▄ █ ▄ ▀   █ ▄ ▀ ▄ ▀ ▄ ▀   ▀ ▄ █   ▀ ▀ ▀ ▀ ▀ ▀ ▀\n        ▄ ▀ █ █ ▄ █ ▀   ▄ ▄ ▄     ▀     ▀ ▀   ▄ ▀ █ █ ▄ █ ▄ ▄ ▄   ▄ ▀ ▀ ▄\n        ▄   █ ▄ ▀ ▄ ▀ ▄   ▄   ▄     █ ▀ ▄ ▀ ▄   ▀ ▀ ▀ █     ▀ ▄ ▀ ▄ ▀ ▀ █\n        ▀ ▄ ▀   █ █ ▀   ▀ ▄ ▄ █ █ ▀   ▄ █ ▄ █ ▀   ▀ ▀ ▄ █ ▀ ▄ █   █ ▄ ▀ ▀\n          █ ▄ ▄     ▀   █ ▀ █   ▄ █ ▀ ▀   ▀ █ ▀ █ ▄ ▀   ▀ ▀ █ █   ▄ █\n          ▄ ▀   ▀   ▀ ▄     ▀   ▀ ▄ █ █ ▀ ▄ █ ▀   █ ▄ ▄ ▀ ▄ █ ▄ █ ▀ ▀ ▄ ▄\n          █ █ █ ▀   ▀ █ ▀ █ ▄   ▀ █ ▄ ▀   ▀     █ ▀   ▄       ▄ █       ▄\n        ▄ ▄ ▄ █ ▀ ▄ ▀ ▀   ▀ █ ▄   ▄   ▄ ▄ █ ▀ ▀ ▄ ▀ ▀ ▄ ▄     ▄   ▄ ▀ ▀ ▄\n          ▄ ▀ █ ▄ █ ▀   █ ▀   ▀ ▄ █       ▀ █     ▀ █ ▀ ▄ ▀ ▄ ▄ ▀ ▄ ▀ █ ▀\n        ▀       ▀   ▀ ▀ ▄ ▄ ▀ ▄ ▀ █   ▀ ▀ ▀ █ █ █ ▄ ▄   █ ▀ ▀ ▀ █ ▄ ▄ █ ▀\n        █ ▀ ▀ ▀ ▀ ▀ █   ▄ ▀ ▄   ▄ ▄ █ ▀ ▄ ▀ ▄ █ ▄ ▄ ▄ ▀ █   ▀   █ █ ▄ ▀\n        █   █ █ █   █   ▄ ▀ ▄ █ █ █ █ ▀ ▀ ▄   █ █   █ █ █ █ ▀ ▀ ▀   █ ▄ █\n        █   ▀ ▀ ▀   █   █ █   █ ▀ ▄ ▄   █ ▄ ▄ ▄ ▄ ▀   █     █   ▄ ▀ ▀   ▄\n        ▀ ▀ ▀ ▀ ▀ ▀ ▀         ▀ ▀ ▀   ▀ ▀         ▀ ▀ ▀     ▀       ▀\n\n\n";
}
