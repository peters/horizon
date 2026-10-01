//! The plan: the steps of what the person asked for and where each stands.
//!
//! The assistant reports it through the `plan` operation of `agent_panels`.
//! Horizon draws the rows, so the statuses are shown as the assistant's own
//! report, never as something Horizon verified.

use egui::{FontId, Margin, Pos2, Rect, RichText, Sense, Shape, Stroke, Ui, vec2};
use horizon_core::browser::manifest::agent_panels::{PlanStep, StepStatus};

use super::HorizonApp;
use super::blocks::{self, Tone};
use super::icons::{self, Icon};
use crate::theme;

const ROW_HEIGHT: f32 = 34.0;
pub(super) const MARKER: f32 = 20.0;

/// Draws the rows of a plan, one line each.
pub(super) fn draw_steps(ui: &mut Ui, steps: &[PlanStep]) {
    for (index, step) in steps.iter().enumerate() {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
        if index + 1 < steps.len() {
            ui.painter()
                .hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, theme::BORDER_SUBTLE()));
        }
        paint_marker(
            ui,
            Rect::from_center_size(rect.left_center() + vec2(MARKER / 2.0, 0.0), vec2(MARKER, MARKER)),
            step.status,
        );
        let title_color = match step.status {
            StepStatus::Pending => theme::FG_DIM(),
            StepStatus::Failed => theme::PALETTE_RED(),
            StepStatus::Running | StepStatus::Done => theme::FG(),
        };
        let painter = ui.painter();
        painter.text(
            rect.left_center() + vec2(MARKER + 12.0, 0.0),
            egui::Align2::LEFT_CENTER,
            &step.title,
            FontId::proportional(13.5),
            title_color,
        );
        if let Some(detail) = &step.detail {
            painter.text(
                rect.right_center(),
                egui::Align2::RIGHT_CENTER,
                detail,
                FontId::monospace(11.5),
                theme::FG_DIM(),
            );
        }
    }
    if steps.iter().any(|step| step.status == StepStatus::Running) {
        ui.ctx().request_repaint();
    }
}

pub(super) fn paint_marker(ui: &Ui, rect: Rect, status: StepStatus) {
    let painter = ui.painter();
    let center = rect.center();
    let radius = MARKER / 2.0;
    match status {
        StepStatus::Done => {
            painter.circle_filled(center, radius, theme::PALETTE_GREEN().gamma_multiply(0.2));
            icons::paint(painter, center, 12.0, Icon::Check, theme::PALETTE_GREEN());
        }
        StepStatus::Failed => {
            painter.circle_filled(center, radius, theme::PALETTE_RED().gamma_multiply(0.2));
            icons::paint(painter, center, 12.0, Icon::Cross, theme::PALETTE_RED());
        }
        StepStatus::Pending => {
            painter.circle_stroke(center, radius - 1.0, Stroke::new(1.5, theme::BORDER_STRONG()));
        }
        StepStatus::Running => {
            // A three-quarter ring that turns while the step runs.
            let start = super::num::seconds(ui) * 4.0;
            let points: Vec<Pos2> = (0..=18_u8)
                .map(|step| {
                    let angle = start + f32::from(step) / 18.0 * std::f32::consts::TAU * 0.75;
                    center + vec2(angle.cos(), angle.sin()) * (radius - 1.0)
                })
                .collect();
            painter.add(Shape::line(points, Stroke::new(2.0, theme::ACCENT())));
        }
    }
}

/// A short line saying how far along the plan is, for headers and chips.
pub(super) fn progress(steps: &[PlanStep]) -> String {
    let done = steps.iter().filter(|step| step.status == StepStatus::Done).count();
    format!("{done} of {} done", steps.len())
}

impl HorizonApp {
    /// The plan as a card in the drawer, above the activity tray.
    pub(super) fn render_plan_block(&mut self, ui: &mut Ui) {
        if self.assistant.plan.is_empty() {
            return;
        }
        egui::Frame::new()
            .inner_margin(Margin::symmetric(14, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                blocks::card(ui, Tone::Neutral, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Plan").size(13.5).strong().color(theme::FG()));
                        ui.label(
                            RichText::new(progress(&self.assistant.plan))
                                .size(11.5)
                                .color(theme::FG_DIM()),
                        );
                    });
                    ui.label(
                        RichText::new("Reported by the assistant")
                            .size(11.0)
                            .color(theme::FG_DIM()),
                    );
                    ui.add_space(4.0);
                    draw_steps(ui, &self.assistant.plan);
                });
            });
    }

    /// Replaces the plan the person sees. An empty plan clears it.
    pub(in crate::app) fn assistant_set_plan(&mut self, steps: Vec<PlanStep>) {
        if let Some(demo) = self.assistant.demo.as_ref() {
            demo.log(&format!("event plan {}", progress(&steps)));
        }
        self.assistant.plan = steps;
    }
}
