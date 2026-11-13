//! The line of a plain SSH panel whose shell is not there.
//!
//! A panel of a plain SSH host keeps its last screen when the link is lost, and
//! nothing starts that panel again by itself: a saved command must not run twice.
//! The person therefore reads here that the link is gone, which keys reach no
//! shell, and where the attempt starts from. A machine terminal says the same in
//! `Connecting...` and `Reconnecting` and reaches its tmux session again on its
//! own, so it needs no button.

use egui::{Align, Layout, Rect, Ui, UiBuilder, Vec2};
use horizon_core::{PanelKind, SshConnectionStatus};

use crate::theme;

/// The height of the strip at the bottom of the grid, in points.
const HEIGHT: f32 = 26.0;
/// The padding inside the strip.
const PADDING: Vec2 = Vec2::new(10.0, 4.0);
/// The size of the text of the line.
const TEXT_SIZE: f32 = 11.5;

/// One line about a panel that has no shell, for the strip under its grid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LinkNotice {
    /// The sentence for the person.
    pub(super) line: String,
    /// Whether the person starts the next attempt, which only a panel that no
    /// timer reaches does.
    pub(super) reconnect: bool,
}

impl LinkNotice {
    /// The line of one connection status of one panel, or `None` while the panel
    /// has a shell or an attempt that reaches it.
    pub(super) fn of(kind: PanelKind, status: Option<SshConnectionStatus>, host: &str) -> Option<Self> {
        if kind != PanelKind::Ssh {
            return None;
        }
        match status? {
            SshConnectionStatus::Disconnected => Some(Self {
                line: format!("The link to {host} is lost. Text typed here is not sent."),
                reconnect: true,
            }),
            SshConnectionStatus::Reconnecting => Some(Self {
                line: format!("Horizon tries {host} again. Text typed here is not sent."),
                reconnect: false,
            }),
            // An attempt of a machine terminal runs its own `ssh`, so text that a
            // person types during it reaches the shell when the link is up.
            SshConnectionStatus::Connecting | SshConnectionStatus::Connected => None,
        }
    }
}

/// Paints the line of `notice` under the grid of `body`, and answers whether the
/// person asked for the next attempt in this pass.
pub(super) fn show(ui: &mut Ui, body: Rect, notice: &LinkNotice, interactive: bool) -> bool {
    let strip = Rect::from_min_max(egui::pos2(body.min.x, (body.max.y - HEIGHT).max(body.min.y)), body.max);
    if !ui.is_rect_visible(strip) {
        return false;
    }
    ui.painter().rect_filled(strip, 0.0, theme::PANEL_BG_ALT());
    let mut reconnect = false;
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(strip.shrink2(PADDING))
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.add(egui::Label::new(
                egui::RichText::new(&notice.line)
                    .color(theme::FG_SOFT())
                    .size(TEXT_SIZE),
            ));
            if notice.reconnect {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let reconnect_button =
                        egui::Button::new(egui::RichText::new("Reconnect").size(TEXT_SIZE).color(theme::FG_SOFT()))
                            .fill(theme::PANEL_BG())
                            .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_SUBTLE()))
                            .corner_radius(10);
                    if ui.add_enabled(interactive, reconnect_button).clicked() {
                        reconnect = true;
                    }
                });
            }
        },
    );
    reconnect
}

#[cfg(test)]
mod tests {
    use horizon_core::{PanelKind, SshConnectionStatus};

    use super::LinkNotice;

    /// B8: a plain SSH panel that lost its link says that the link is gone and
    /// that typed text reaches no shell. The sentence is the only thing that tells
    /// a person, because the panel keeps its last screen and nothing else changes.
    #[test]
    fn a_lost_link_says_that_typed_text_is_not_sent() {
        let notice = LinkNotice::of(PanelKind::Ssh, Some(SshConnectionStatus::Disconnected), "lab-1")
            .expect("a disconnected ssh panel says a line");
        assert_eq!(notice.line, "The link to lab-1 is lost. Text typed here is not sent.");
        assert!(
            notice.reconnect,
            "no timer reaches a plain SSH panel, so the person does"
        );
    }

    #[test]
    fn a_panel_that_waits_for_its_attempt_says_the_same_and_needs_no_button() {
        let notice = LinkNotice::of(PanelKind::Ssh, Some(SshConnectionStatus::Reconnecting), "lab-1")
            .expect("a waiting terminal says a line");
        assert_eq!(notice.line, "Horizon tries lab-1 again. Text typed here is not sent.");
        assert!(
            !notice.reconnect,
            "the wait of a machine terminal ends by itself, so the panel offers no second way"
        );
    }

    #[test]
    fn a_panel_with_a_shell_or_an_attempt_has_no_line() {
        for status in [SshConnectionStatus::Connected, SshConnectionStatus::Connecting] {
            assert!(
                LinkNotice::of(PanelKind::Ssh, Some(status), "lab-1").is_none(),
                "{status:?} sends text to a shell"
            );
        }
        assert!(LinkNotice::of(PanelKind::Shell, Some(SshConnectionStatus::Disconnected), "lab-1").is_none());
        assert!(LinkNotice::of(PanelKind::Ssh, None, "lab-1").is_none());
    }
}
