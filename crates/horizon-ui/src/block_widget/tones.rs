//! What a tone means in colour.
//!
//! A tone never names a Horizon state: it only picks one of the theme's state
//! colours, so the same document is drawn correctly in both themes. Everything
//! here is a pure function of the tone and the active theme, which makes the
//! mapping testable without a frame.

use egui::Color32;
use horizon_browser_control::block::Tone;

use crate::theme;

use super::metrics;

/// The colours one badge or one status frame is drawn with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ToneColors {
    /// Text and, for a state tone, the dot.
    pub(super) text: Color32,
    /// Fill behind the text.
    pub(super) fill: Color32,
    /// One pixel outline.
    pub(super) outline: Color32,
}

/// The colours one button is drawn with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ButtonColors {
    /// Label colour.
    pub(super) text: Color32,
    /// Fill behind the label.
    pub(super) fill: Color32,
    /// One pixel outline.
    pub(super) outline: Color32,
}

/// Alpha of the tint fill of a status frame and of a danger button.
const TINT_ALPHA: u8 = 23;

/// The state colour of a tone. `neutral` has none, so it takes secondary text.
pub(super) fn text(tone: Tone) -> Color32 {
    match tone {
        Tone::Neutral => theme::FG_SOFT(),
        Tone::Info => theme::PALETTE_CYAN(),
        Tone::Ok => theme::PALETTE_GREEN(),
        Tone::Warn => theme::PALETTE_YELLOW(),
        Tone::Danger => theme::PALETTE_RED(),
        Tone::Primary => theme::ACCENT(),
    }
}

/// The colours of a badge. A state tone gets a translucent fill, an outline at
/// a third of its colour and a dot; the neutral tone is a plain raised pill
/// with no dot, so it never looks like a state.
pub(super) fn badge(tone: Tone) -> ToneColors {
    let text = text(tone);
    if tone == Tone::Neutral {
        return ToneColors {
            text,
            fill: theme::PANEL_BG_ALT(),
            outline: theme::BORDER_SUBTLE(),
        };
    }
    ToneColors {
        text,
        fill: theme::alpha(text, metrics::BADGE_FILL_ALPHA),
        outline: theme::alpha(text, metrics::BADGE_OUTLINE_ALPHA),
    }
}

/// Whether the badge of `tone` carries a dot.
pub(super) fn badge_has_dot(tone: Tone) -> bool {
    tone != Tone::Neutral
}

/// The colours of a status frame: a translucent tint of the tone, an outline at
/// a third of its colour, and the tone itself for the strong line.
pub(super) fn status(tone: Tone) -> ToneColors {
    let text = text(tone);
    ToneColors {
        text,
        fill: theme::alpha(text, TINT_ALPHA),
        outline: theme::alpha(text, metrics::BADGE_OUTLINE_ALPHA),
    }
}

/// The colours of a button. A disabled button is a raised fill with dim text
/// whatever its tone, so a disabled primary action never reads as the next
/// step. The primary fill is solid `ACCENT` with `BG` text, the one pair that
/// passes on both themes.
pub(super) fn button(tone: Tone, enabled: bool) -> ButtonColors {
    if !enabled {
        return ButtonColors {
            text: theme::FG_DIM(),
            fill: theme::PANEL_BG_ALT(),
            outline: theme::BORDER_SUBTLE(),
        };
    }
    match tone {
        Tone::Primary => ButtonColors {
            text: theme::BG(),
            fill: theme::ACCENT(),
            outline: theme::ACCENT(),
        },
        Tone::Danger => ButtonColors {
            text: theme::PALETTE_RED(),
            fill: theme::alpha(theme::PALETTE_RED(), TINT_ALPHA),
            outline: theme::alpha(theme::PALETTE_RED(), metrics::BADGE_OUTLINE_ALPHA),
        },
        Tone::Neutral | Tone::Info | Tone::Ok | Tone::Warn => ButtonColors {
            text: theme::FG(),
            fill: theme::PANEL_BG_ALT(),
            outline: theme::BORDER_SUBTLE(),
        },
    }
}

#[cfg(test)]
mod tests {
    use horizon_browser_control::block::Tone;

    use crate::theme;

    use super::{badge, badge_has_dot, button, status, text};

    fn with_theme<R>(resolved: theme::ResolvedTheme, run: impl FnOnce() -> R) -> R {
        let previous = theme::current_theme();
        theme::set_theme(resolved);
        let value = run();
        theme::set_theme(previous);
        value
    }

    #[test]
    fn every_state_tone_picks_its_state_colour_in_both_themes() {
        for resolved in [theme::ResolvedTheme::Dark, theme::ResolvedTheme::Light] {
            with_theme(resolved, || {
                assert_eq!(text(Tone::Ok), theme::PALETTE_GREEN());
                assert_eq!(text(Tone::Warn), theme::PALETTE_YELLOW());
                assert_eq!(text(Tone::Danger), theme::PALETTE_RED());
                assert_eq!(text(Tone::Info), theme::PALETTE_CYAN());
                assert_eq!(text(Tone::Primary), theme::ACCENT());
                assert_eq!(text(Tone::Neutral), theme::FG_SOFT());
            });
        }
    }

    #[test]
    fn a_state_badge_is_a_translucent_tint_with_a_dot() {
        with_theme(theme::ResolvedTheme::Dark, || {
            let colours = badge(Tone::Ok);
            assert_eq!(colours.text, theme::PALETTE_GREEN());
            assert_eq!(colours.fill, theme::alpha(theme::PALETTE_GREEN(), 28));
            assert_eq!(colours.outline, theme::alpha(theme::PALETTE_GREEN(), 90));
            assert!(badge_has_dot(Tone::Ok));
        });
    }

    #[test]
    fn a_neutral_badge_is_a_raised_pill_without_a_dot() {
        for resolved in [theme::ResolvedTheme::Dark, theme::ResolvedTheme::Light] {
            with_theme(resolved, || {
                let colours = badge(Tone::Neutral);
                assert_eq!(colours.fill, theme::PANEL_BG_ALT());
                assert_eq!(colours.outline, theme::BORDER_SUBTLE());
                assert_eq!(colours.text, theme::FG_SOFT());
                assert!(!badge_has_dot(Tone::Neutral));
            });
        }
    }

    #[test]
    fn a_status_frame_tints_its_tone() {
        with_theme(theme::ResolvedTheme::Light, || {
            let colours = status(Tone::Warn);
            assert_eq!(colours.fill, theme::alpha(theme::PALETTE_YELLOW(), 23));
            assert_eq!(colours.outline, theme::alpha(theme::PALETTE_YELLOW(), 90));
            assert_eq!(colours.text, theme::PALETTE_YELLOW());
        });
    }

    #[test]
    fn a_primary_button_is_a_solid_accent_fill_with_canvas_text() {
        for resolved in [theme::ResolvedTheme::Dark, theme::ResolvedTheme::Light] {
            with_theme(resolved, || {
                let colours = button(Tone::Primary, true);
                assert_eq!(colours.fill, theme::ACCENT());
                assert_eq!(colours.text, theme::BG());
                assert_eq!(colours.outline, theme::ACCENT());
            });
        }
    }

    #[test]
    fn a_danger_button_is_red_text_on_a_red_tint() {
        with_theme(theme::ResolvedTheme::Dark, || {
            let colours = button(Tone::Danger, true);
            assert_eq!(colours.text, theme::PALETTE_RED());
            assert_eq!(colours.fill, theme::alpha(theme::PALETTE_RED(), 23));
            assert_eq!(colours.outline, theme::alpha(theme::PALETTE_RED(), 90));
        });
    }

    #[test]
    fn a_disabled_button_is_dim_whatever_its_tone() {
        for tone in [
            Tone::Neutral,
            Tone::Info,
            Tone::Ok,
            Tone::Warn,
            Tone::Danger,
            Tone::Primary,
        ] {
            let colours = button(tone, false);
            assert_eq!(colours.text, theme::FG_DIM(), "{tone:?}");
            assert_eq!(colours.fill, theme::PANEL_BG_ALT(), "{tone:?}");
            assert_eq!(colours.outline, theme::BORDER_SUBTLE(), "{tone:?}");
        }
    }

    #[test]
    fn a_secondary_button_is_the_raised_fill() {
        for tone in [Tone::Neutral, Tone::Info, Tone::Ok, Tone::Warn] {
            let colours = button(tone, true);
            assert_eq!(colours.fill, theme::PANEL_BG_ALT(), "{tone:?}");
            assert_eq!(colours.text, theme::FG(), "{tone:?}");
            assert_eq!(colours.outline, theme::BORDER_SUBTLE(), "{tone:?}");
        }
    }
}
