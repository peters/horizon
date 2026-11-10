//! The state chip in the titlebar of a card.
//!
//! The chip is Horizon's answer to "will a press do anything?". It names the
//! agent that owns the card and says whether that agent waits for a click right
//! now, so a person can see that a click will be read before making one. The
//! document cannot set it.

use egui::{CornerRadius, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, Ui};
use horizon_core::PanelKind;

use crate::theme;

use super::metrics::{CHIP_DOT, CHIP_DOT_GAP, CHIP_FONT, CHIP_HEIGHT, CHIP_PAD_SIDE, CHIP_RADIUS, CHIP_TITLE_GAP};
use super::text;

/// What the chip says about the owner of a card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChipState {
    /// A `next_click` of the owner waits, so a press reaches it at once.
    Listening,
    /// An agent panel of the owner is in the workspace but not waiting.
    Present,
    /// No agent panel of the owner is in the workspace.
    Gone,
    /// The person's panel was opened for them by this agent.
    OpenedBy,
}

/// The text the chip adds after the agent name while a wait is open.
const LISTENS: &str = " listens";
/// The chip text when the owner is gone.
const NO_AGENT: &str = "No agent";
/// The words that mark a panel that an agent opened for the person.
const OPENED: &str = "opened by ";

/// Space the title keeps from the chip while both are drawn.
pub(crate) const TITLE_GAP: f32 = CHIP_TITLE_GAP;

/// Height of the chip.
pub(crate) const fn height() -> f32 {
    CHIP_HEIGHT
}

/// Width `card` needs, its dot and padding included.
pub(crate) fn width(ui: &Ui, card: &CardChip<'_>) -> f32 {
    card.width(ui)
}

/// The chip of one card: the owner's name and what it is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CardChip<'a> {
    /// Display name of the owning panel kind, such as `Claude`.
    pub(crate) name: &'a str,
    /// Whether the owner waits for a click.
    pub(crate) state: ChipState,
}

impl<'a> CardChip<'a> {
    /// The chip of a card whose owner panel is `kind`, or the chip of a card
    /// whose owner is gone.
    pub(crate) fn for_panel(kind: PanelKind, listening: bool, present: bool) -> Self {
        Self {
            name: kind.display_name(),
            state: if !present {
                ChipState::Gone
            } else if listening {
                ChipState::Listening
            } else {
                ChipState::Present
            },
        }
    }

    /// The mark of a panel that an agent opened for the person, naming that
    /// agent. The mark is Horizon's own text: a panel title or a command cannot
    /// set it.
    pub(crate) fn opened_by(name: &'a str) -> Self {
        Self {
            name,
            state: ChipState::OpenedBy,
        }
    }

    /// Width the chip needs, its dot and padding included.
    fn width(&self, ui: &Ui) -> f32 {
        let font = FontId::monospace(CHIP_FONT);
        let mut width = 2.0 * CHIP_PAD_SIDE + CHIP_DOT + CHIP_DOT_GAP;
        match self.state {
            ChipState::Listening => {
                width += text::line(ui.painter(), self.name, &font, theme::FG_SOFT(), f32::INFINITY)
                    .size()
                    .x;
                width += text::line(ui.painter(), LISTENS, &font, theme::FG_SOFT(), f32::INFINITY)
                    .size()
                    .x;
            }
            ChipState::Present | ChipState::OpenedBy => {
                width += text::line(ui.painter(), self.name, &font, theme::FG_SOFT(), f32::INFINITY)
                    .size()
                    .x;
            }
            ChipState::Gone => {
                width += text::line(ui.painter(), NO_AGENT, &font, theme::FG_SOFT(), f32::INFINITY)
                    .size()
                    .x;
            }
        }
        if self.state == ChipState::OpenedBy {
            width += text::line(ui.painter(), OPENED, &font, theme::FG_SOFT(), f32::INFINITY)
                .size()
                .x;
        }
        width
    }

    /// The text the chip says, as a reader or a test hears it: the name and the
    /// words that follow it, and the words that come before it for a marked
    /// panel.
    pub(crate) fn spoken(&self) -> (&'static str, &'static str, &'a str) {
        match self.state {
            ChipState::Listening => ("", LISTENS, self.name),
            ChipState::Present => ("", "", self.name),
            ChipState::Gone => ("", "", NO_AGENT),
            ChipState::OpenedBy => (OPENED, "", self.name),
        }
    }

    /// Paint the chip inside `rect`.
    pub(crate) fn paint(&self, painter: &Painter, rect: Rect) {
        let radius = CornerRadius::same(CHIP_RADIUS);
        painter.rect_filled(rect, radius, theme::BG_ELEVATED());
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(1.0, theme::BORDER_SUBTLE()),
            StrokeKind::Inside,
        );
        let font = FontId::monospace(CHIP_FONT);
        let center = rect.center();
        let dot = Pos2::new(rect.min.x + CHIP_PAD_SIDE + CHIP_DOT * 0.5, center.y);
        match self.state {
            ChipState::Listening => {
                painter.circle_filled(dot, CHIP_DOT * 0.5, theme::PALETTE_GREEN());
            }
            ChipState::Present | ChipState::Gone | ChipState::OpenedBy => {
                painter.circle_stroke(dot, CHIP_DOT * 0.5 - 0.75, Stroke::new(1.5, theme::FG_SOFT()));
            }
        }
        let mut x = rect.min.x + CHIP_PAD_SIDE + CHIP_DOT + CHIP_DOT_GAP;
        for part in self.parts().into_iter().filter(|part| !part.is_empty()) {
            let galley = text::line(painter, part, &font, theme::FG_SOFT(), f32::INFINITY);
            let height = galley.size().y;
            let width = galley.size().x;
            painter.galley(Pos2::new(x, center.y - height * 0.5), galley, theme::FG_SOFT());
            x += width;
        }
    }

    /// The parts the chip draws, in order, with no string built for them.
    fn parts(&self) -> [&str; 3] {
        let (prefix, suffix, name) = self.spoken();
        [prefix, name, suffix]
    }
}
