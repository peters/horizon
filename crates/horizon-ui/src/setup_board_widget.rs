//! The setup board: one row for each repository whose cloud setup agent works on this
//! computer, with its step progress, what it does now and its actions.
//!
//! It only draws the rows the app filled in. Each button is a request the app carries
//! out after drawing.
use egui::{Align, Color32, CornerRadius, Frame, Layout, Margin, RichText, ScrollArea, Sense, Stroke, Ui, Vec2};
use horizon_core::Panel;
use horizon_core::setup_board::{Request, Row, RowState, SetupBoard, Step};

use crate::theme;

/// The width of one step's segment in a row's progress bar.
const SEGMENT_WIDTH: f32 = 18.0;
const SEGMENT_HEIGHT: f32 = 6.0;
const SEGMENT_GAP: f32 = 3.0;

pub struct SetupBoardView<'a> {
    panel: &'a mut Panel,
}

impl<'a> SetupBoardView<'a> {
    pub fn new(panel: &'a mut Panel) -> Self {
        Self { panel }
    }

    /// Draws the board. Returns true when the pointer is over it, for focus tracking.
    pub fn show(&mut self, ui: &mut Ui, _is_focused: bool) -> bool {
        let hovered = ui.rect_contains_pointer(ui.max_rect());
        let panel_id = self.panel.id.0;
        let Some(board) = self.panel.content.setup_board_mut() else {
            return hovered;
        };
        let mut request = None;
        ScrollArea::vertical()
            .id_salt(("setup-board", panel_id))
            .auto_shrink(false)
            .show(ui, |ui| {
                Frame::new().inner_margin(Margin::symmetric(16, 14)).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                    request = header(ui, board);
                    ui.add_space(4.0);
                    if board.rows.is_empty() {
                        ui.label(
                            RichText::new(
                                "No setup works now. Set up a repository: an agent writes its cloud settings from its CI, and Horizon proves them on this computer.",
                            )
                            .size(12.5)
                            .color(theme::FG_SOFT()),
                        );
                    }
                    for row in &board.rows {
                        if let Some(pressed) = row_card(ui, row) {
                            request = Some(pressed);
                        }
                    }
                });
            });
        if request.is_some() {
            board.request = request;
        }
        hovered
    }
}

/// How many repositories the board shows and how many work, and the button that starts
/// another setup.
fn header(ui: &mut Ui, board: &SetupBoard) -> Option<Request> {
    let rows = &board.rows;
    let (working, waiting) = board.busy();
    let mut request = None;
    ui.horizontal(|ui| {
        let count = match rows.len() {
            1 => "1 repository".to_owned(),
            count => format!("{count} repositories"),
        };
        ui.label(RichText::new(count).size(14.0).strong().color(theme::FG()));
        if working + waiting > 0 {
            let busy = if waiting > 0 {
                format!("{working} working · {waiting} waiting for a build slot")
            } else {
                format!("{working} working")
            };
            ui.label(RichText::new(busy).size(12.5).color(theme::FG_SOFT()));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let button = egui::Button::new(
                RichText::new("Set up a repository")
                    .size(13.0)
                    .strong()
                    .color(theme::BG()),
            )
            .fill(theme::ACCENT())
            .corner_radius(8)
            .min_size(Vec2::new(0.0, 32.0));
            if ui
                .add(button)
                .on_hover_text("Open New cloud to choose a repository and start its setup agent.")
                .clicked()
            {
                request = Some(Request::NewSetup);
            }
        });
    });
    request
}

/// One repository: its name and path, its steps, what it does now and its buttons.
fn row_card(ui: &mut Ui, row: &Row) -> Option<Request> {
    let tint = state_color(row.state);
    let mut request = None;
    Frame::new()
        .fill(theme::blend(theme::PANEL_BG_ALT(), tint, 0.05))
        .stroke(Stroke::new(1.0, theme::blend(theme::BORDER_SUBTLE(), tint, 0.3)))
        .corner_radius(10)
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(&row.name).size(14.0).strong().color(theme::FG()));
                ui.label(RichText::new(&row.agent).size(11.5).color(theme::FG_SOFT()));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    request = buttons(ui, row);
                });
            });
            ui.add(
                egui::Label::new(RichText::new(&row.path).size(11.5).monospace().color(theme::FG_SOFT())).truncate(),
            );
            ui.add_space(2.0);
            progress(ui, &row.steps);
            let color = if row.alert {
                theme::PALETTE_RED()
            } else {
                now_color(row.state)
            };
            ui.add(egui::Label::new(RichText::new(&row.now).size(12.5).color(color)).truncate())
                .on_hover_text(&row.now);
        });
    request
}

/// Open agent, Open in New cloud and Stop, right to left.
fn buttons(ui: &mut Ui, row: &Row) -> Option<Request> {
    let mut request = None;
    // A stopped run only leaves the board; Stop waits while Use commits.
    let (label, hint) = if row.state == RowState::Stopped {
        (
            "Remove",
            "Take this setup off the board and remove what its proof made.",
        )
    } else if row.committing {
        (
            "Stop",
            "Use this setup commits the setup now; Stop waits until it reported.",
        )
    } else {
        ("Stop", "Stop the agent and remove what its proof made.")
    };
    let stop = ui
        .add_enabled(
            !row.committing,
            row_button(theme::PALETTE_RED(), theme::PALETTE_RED(), label),
        )
        .on_hover_text(hint)
        .on_disabled_hover_text(hint);
    if stop.clicked() {
        request = Some(Request::Stop(row.run));
    }
    let open_label = if row.state == RowState::Ready {
        "Use in New cloud"
    } else {
        "Open in New cloud"
    };
    if ui
        .add(row_button(theme::ACCENT(), theme::FG_SOFT(), open_label))
        .on_hover_text("Open New cloud for this repository: its steps, the agent's terminal and Use this setup.")
        .clicked()
    {
        request = Some(Request::OpenInNewCloud(row.run));
    }
    if row.agent_open
        && ui
            .add(row_button(theme::ACCENT(), theme::FG_SOFT(), "Open agent"))
            .on_hover_text("Show the setup agent's panel.")
            .clicked()
    {
        request = Some(Request::OpenAgent(row.run));
    }
    request
}

/// One segment for each step, colored by its state; the whole bar names the states.
fn progress(ui: &mut Ui, steps: &[Step]) {
    let width = steps
        .iter()
        .fold(-SEGMENT_GAP, |width, _| width + SEGMENT_WIDTH + SEGMENT_GAP)
        .max(0.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, SEGMENT_HEIGHT), Sense::hover());
    let done = steps
        .iter()
        .filter(|step| matches!(step, Step::Done | Step::Skipped))
        .count();
    response.on_hover_text(format!("{done} of {} steps done", steps.len()));
    if !ui.is_rect_visible(rect) {
        return;
    }
    let mut left = rect.left();
    for step in steps {
        let segment = egui::Rect::from_min_size(egui::pos2(left, rect.top()), Vec2::new(SEGMENT_WIDTH, SEGMENT_HEIGHT));
        ui.painter()
            .rect_filled(segment, CornerRadius::same(3), step_color(*step));
        left += SEGMENT_WIDTH + SEGMENT_GAP;
    }
}

/// A row's button, in the cloud card style.
fn row_button(color: Color32, text: Color32, label: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(label).size(12.5).color(text))
        .min_size(Vec2::new(0.0, 30.0))
        .fill(theme::alpha(theme::blend(theme::PANEL_BG_ALT(), color, 0.05), 220))
        .stroke(Stroke::new(
            1.0,
            theme::alpha(theme::blend(theme::BORDER_SUBTLE(), color, 0.24), 216),
        ))
        .corner_radius(8)
}

fn step_color(step: Step) -> Color32 {
    match step {
        Step::Done => theme::PALETTE_GREEN(),
        Step::Skipped => theme::blend(theme::PALETTE_GREEN(), theme::BORDER_SUBTLE(), 0.5),
        Step::Running => theme::ACCENT(),
        Step::Waiting => theme::PALETTE_YELLOW(),
        Step::Failed => theme::PALETTE_RED(),
        Step::Pending => theme::BORDER_SUBTLE(),
    }
}

fn state_color(state: RowState) -> Color32 {
    match state {
        RowState::Working => theme::ACCENT(),
        RowState::Waiting => theme::PALETTE_YELLOW(),
        RowState::Ready => theme::PALETTE_GREEN(),
        RowState::Failed => theme::PALETTE_RED(),
        RowState::Stopped => theme::BORDER_SUBTLE(),
    }
}

fn now_color(state: RowState) -> Color32 {
    match state {
        RowState::Waiting => theme::PALETTE_YELLOW(),
        RowState::Failed => theme::PALETTE_RED(),
        RowState::Ready => theme::PALETTE_GREEN(),
        RowState::Working | RowState::Stopped => theme::FG_SOFT(),
    }
}
