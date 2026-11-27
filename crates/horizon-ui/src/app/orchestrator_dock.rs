//! The orchestrator dock: the column at the right edge of the root window that
//! holds the conversation with the orchestrator of a workspace.
//!
//! Only its frame is here: the switch of the experiment, the rectangle of the
//! frame, the header, the empty state and the composer. No agent, no process and
//! no message is part of this part, so the send control is disabled and the dock
//! draws only after the person turns the experiment on.
//!
//! The dock keeps the right edge of the window, and an open Settings panel stands
//! to its left, so the canvas ends at the left edge of the leftmost of the two.
//! Nothing here is saved: the switch, the rectangle of a frame and the drafts are
//! the state of this run, as `sidebar_visible` and `minimap_visible` are.

mod composer;
mod header;
mod layout;
#[cfg(test)]
mod tests;
mod text;

use std::collections::HashMap;

use egui::{Rect, Stroke, Ui, pos2};

use crate::app::settings::settings_panel_default_width;
use crate::app::util::viewport_local_rect;
use crate::app::{HorizonApp, SIDEBAR_WIDTH, TOOLBAR_HEIGHT};
use crate::theme;
use crate::typeface::Weight;

use self::layout::{dock_fits, dock_rect, dock_width};

/// The environment switch of this part. `1` turns the dock on; a setting replaces
/// the variable in a later part.
pub(in crate::app) const DOCK_SWITCH: &str = "HORIZON_ORCHESTRATOR_DOCK";
/// The panel id of the dock, which egui remembers the place of.
const DOCK_PANEL_ID: &str = "orchestrator_dock";
/// The empty state of the dock: the least horizontal padding, the title and the
/// two sentences under it.
const EMPTY_PADDING_X: f32 = 28.0;
const EMPTY_TITLE: &str = "Say what you want done.";
const EMPTY_TITLE_SIZE: f32 = 15.0;
const EMPTY_BODY: &str = "The orchestrator reads the repository, makes a plan and shows it as cards. \
                          It asks before a machine is rented.";
const EMPTY_BODY_SIZE: f32 = 13.5;
const EMPTY_BODY_LINE_HEIGHT: f32 = 1.55;
const EMPTY_GAP: f32 = 8.0;

/// The state of the dock of this run.
#[derive(Default)]
pub(in crate::app) struct OrchestratorDock {
    /// Whether the switch of the experiment is on.
    visible: bool,
    /// The rectangle that the dock took in this frame, from before it drew.
    rect: Option<Rect>,
    /// The rectangles that the dock worked out in the last frame of a test, which
    /// the tests of the layout read.
    #[cfg(test)]
    last_frame: Option<LastFrame>,
    /// The draft of each workspace, by the `local_id` that a restart keeps. A
    /// `WorkspaceId` is new at every start, so it cannot be the key.
    drafts: HashMap<String, String>,
}

/// The rectangles of the parts of the dock in one frame, which a test reads.
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(in crate::app) struct LastFrame {
    pub(in crate::app) header: Rect,
    pub(in crate::app) body: Rect,
    pub(in crate::app) composer: Rect,
    pub(in crate::app) field: Rect,
    pub(in crate::app) send: Rect,
    pub(in crate::app) chips: Rect,
}

impl OrchestratorDock {
    /// The dock of a start, as the environment switch names it.
    pub(in crate::app) fn from_environment() -> Self {
        Self::new(dock_switch(std::env::var(DOCK_SWITCH).ok().as_deref()))
    }

    pub(in crate::app) fn new(visible: bool) -> Self {
        Self {
            visible,
            ..Self::default()
        }
    }

    pub(in crate::app) fn visible(&self) -> bool {
        self.visible
    }

    pub(in crate::app) fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    /// The rectangle of the dock in this frame. A frame that draws no dock has
    /// none, so the canvas keeps the whole window.
    pub(in crate::app) fn frame_rect(&self) -> Option<Rect> {
        self.rect
    }

    pub(in crate::app) fn set_frame_rect(&mut self, rect: Option<Rect>) {
        self.rect = rect;
    }

    /// The draft of the workspace that `local_id` names, which is empty until the
    /// person writes in it.
    fn draft(&self, local_id: &str) -> &str {
        self.drafts.get(local_id).map_or("", String::as_str)
    }

    /// The draft of the workspace that `local_id` names, which is empty until the
    /// person writes in it.
    fn draft_mut(&mut self, local_id: &str) -> &mut String {
        self.drafts.entry(local_id.to_owned()).or_default()
    }

    /// Keeps the rectangles of this frame for the tests of the layout.
    #[cfg(test)]
    fn set_last_frame(&mut self, parts: layout::Parts, composer: composer::Parts) {
        self.last_frame = Some(LastFrame {
            header: parts.header,
            body: parts.body,
            composer: parts.composer,
            field: composer.field,
            send: composer.send,
            chips: composer.chips,
        });
    }

    /// The rectangles of the parts of the last frame, which a test reads to
    /// assert where they stand.
    #[cfg(test)]
    pub(in crate::app) fn last_frame(&self) -> Option<LastFrame> {
        self.last_frame
    }
}

/// The panel of the right edge that the canvas must stop at: the leftmost of the
/// panels that stand there, which is the dock or an open Settings panel.
pub(in crate::app) fn leftmost_panel(panels: [Option<Rect>; 2]) -> Option<Rect> {
    panels
        .into_iter()
        .flatten()
        .reduce(|left, rect| if rect.min.x < left.min.x { rect } else { left })
}

/// Whether the switch of the dock is on: the variable holds `1` and nothing else.
fn dock_switch(value: Option<&str>) -> bool {
    value == Some("1")
}

impl HorizonApp {
    /// The rectangle that the dock takes in this frame, before it draws: the right
    /// edge of the window, from under the toolbar to its bottom. The answer is
    /// `None` while the switch is off, a panel is fullscreen, or the canvas would
    /// keep less than its least width beside the dock.
    fn orchestrator_dock_rect(&self, ui: &Ui) -> Option<Rect> {
        if !self.orchestrator_dock.visible() || self.fullscreen_panel.is_some() {
            return None;
        }
        let viewport = viewport_local_rect(ui.ctx());
        let available = ui.available_rect_before_wrap();
        let width = dock_width(viewport.width()).min(available.width());
        let sidebar_width = if self.sidebar_visible { SIDEBAR_WIDTH } else { 0.0 };
        let settings_width = self
            .settings
            .as_ref()
            .map_or(0.0, |_| settings_panel_default_width(viewport.width()));
        dock_fits(viewport.width(), sidebar_width, settings_width, width)
            .then(|| dock_rect(Rect::from_min_max(viewport.min, available.max), width))
    }

    /// Takes the rectangle of the dock for this frame and draws its frame. The
    /// rectangle is taken before any panel draws, so the canvas and the minimap of
    /// this frame read it.
    pub(in crate::app) fn render_orchestrator_dock(&mut self, ui: &mut Ui) {
        let dock_rect = self.orchestrator_dock_rect(ui);
        self.orchestrator_dock.set_frame_rect(dock_rect);
        let Some(rect) = dock_rect else {
            return;
        };
        let board = &self.board;
        let dock = &mut self.orchestrator_dock;
        egui::Panel::right(DOCK_PANEL_ID)
            .exact_size(rect.width())
            .frame(
                egui::Frame::default()
                    .fill(theme::BG_ELEVATED())
                    .stroke(Stroke::new(1.0_f32, theme::BORDER_SUBTLE())),
            )
            .show(ui, |ui| {
                ui.shrink_clip_rect(ui.max_rect());
                // The toolbar of the window draws over the top of every panel, so
                // the content of the dock starts under it.
                let content = Rect::from_min_max(
                    pos2(ui.max_rect().min.x, ui.max_rect().min.y + TOOLBAR_HEIGHT),
                    ui.max_rect().max,
                );
                let workspace = board.active_workspace.and_then(|id| board.workspace(id));
                // The header and the composer are bottom and top edges of the dock,
                // so they are worked out first and the body takes the room between
                // them. A part that drew top down would move the parts under it.
                let header_height = header::height(ui, content.width(), workspace);
                let draft = workspace.map_or("", |workspace| dock.draft(workspace.local_id.as_str()));
                let composer = composer::parts(ui, content, draft);
                let parts = layout::parts(content, header_height, composer.rect.height());
                header::render(ui, parts.header, workspace);
                let mut empty = String::new();
                let draft = match workspace {
                    Some(workspace) => dock.draft_mut(workspace.local_id.as_str()),
                    None => &mut empty,
                };
                composer::render(ui, &composer, draft, workspace.is_some());
                empty_state(ui, parts.body);
                #[cfg(test)]
                dock.set_last_frame(parts, composer);
            });
    }
}

/// The empty state of the dock: how the orchestrator works, centred in the `body`
/// between the header and the composer.
fn empty_state(ui: &Ui, body: Rect) {
    let text_width = body.width() - 2.0 * EMPTY_PADDING_X;
    if text_width <= 0.0 || body.height() <= 0.0 {
        return;
    }
    let title = text::line(ui, EMPTY_TITLE, EMPTY_TITLE_SIZE, Weight::Bold, theme::FG(), text_width);
    let body_text = text::paragraph(
        ui,
        EMPTY_BODY,
        EMPTY_BODY_SIZE,
        Weight::Regular,
        theme::FG_SOFT(),
        text_width,
        EMPTY_BODY_LINE_HEIGHT,
    );
    let title_height = title.size().y;
    let height = title_height + EMPTY_GAP + body_text.size().y;
    let left = body.min.x + EMPTY_PADDING_X;
    let top = body.center().y - height / 2.0;
    if top < body.min.y || top + height > body.max.y {
        return;
    }
    let painter = ui.painter();
    painter.galley(pos2(left, top), title, theme::FG());
    painter.galley(pos2(left, top + title_height + EMPTY_GAP), body_text, theme::FG_SOFT());
}
