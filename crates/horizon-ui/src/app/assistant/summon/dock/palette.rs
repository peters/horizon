//! Design 3, the palette: an orb in the corner, and a centred panel that opens over a dimmed canvas.
//!
//! Mini: just the orb at the right edge, with a short caption bubble only when there is something to
//! say. Expanded: a command-palette panel. The prompt comes first and large, the workspace chips under
//! it, and the conversation as cards below.

use egui::{
    Align2, Area, Color32, Context, CornerRadius, Frame, Id, Margin, Order, Rect, Sense, Shadow, Stroke, pos2, vec2,
};

use super::super::super::{icons, num};
use super::super::mini::MiniAction;
use super::super::{Action, HorizonApp, INPUT_ID, demo};
use crate::theme;

const ORB_BLOCK: f32 = 104.0;
const CAPTION_WIDTH: f32 = 380.0;
const MODAL_WIDTH: f32 = 780.0;

impl HorizonApp {
    pub(super) fn palette_mini(&mut self, ctx: &Context) {
        let canvas = self.canvas_rect(ctx);
        let agents = self.feed_agents();
        let has_caption = self.palette_has_news(&agents);
        let width = if has_caption {
            ORB_BLOCK + CAPTION_WIDTH
        } else {
            ORB_BLOCK
        };
        let mut actions: Vec<MiniAction> = Vec::new();
        let area = Area::new(Id::new("assistant_dock"))
            .order(Order::Tooltip)
            .pivot(Align2::RIGHT_BOTTOM)
            // Above the minimap that sits in the corner.
            .fixed_pos(pos2(canvas.right() - 18.0, canvas.bottom() - 190.0))
            .show(ctx, |ui| {
                let (whole, _) = ui.allocate_exact_size(vec2(width, ORB_BLOCK), Sense::hover());
                let now = num::seconds(ui);
                let centre = pos2(whole.right() - 52.0, whole.center().y);
                self.paint_orb(ui, centre, 34.0, &agents, now);
                if ui
                    .interact(
                        Rect::from_center_size(centre, vec2(70.0, 70.0)),
                        Id::new("palette_orb"),
                        Sense::click(),
                    )
                    .on_hover_text("Ask the assistant")
                    .clicked()
                {
                    actions.push(MiniAction::Expand);
                }
                if has_caption {
                    let caption = Rect::from_min_max(
                        pos2(whole.left(), whole.top() + 16.0),
                        pos2(centre.x - 44.0, whole.bottom() - 16.0),
                    );
                    self.orb_caption(ui, caption, &agents, now, false, &mut actions);
                }
            });
        ctx.move_to_top(area.response.layer_id);
        self.assistant.summon.rect = Some(area.response.rect);
        for action in actions {
            self.apply_dock_action(ctx, &action);
        }
    }

    /// Something worth a bubble: a question, speech, dictation or work under way.
    fn palette_has_news(&self, agents: &[super::super::feed::AgentRow]) -> bool {
        agents
            .iter()
            .any(|agent| agent.state == horizon_core::browser::manifest::agent_panels::AgentState::NeedsInput)
            || self.speaking_now()
            || self.assistant.demo.as_ref().is_some_and(demo::Demo::is_listening)
            || self.assistant_busy()
    }

    pub(super) fn palette_modal(&mut self, ctx: &Context) {
        let canvas = self.canvas_rect(ctx);
        let width = MODAL_WIDTH.min(canvas.width() - 48.0).max(420.0);
        let height = (canvas.height() * 0.8).min(680.0);
        let tiles = self.dock_tiles();
        let mut close = false;
        // The canvas dims; a click on it puts the palette away.
        Area::new(Id::new("assistant_dock_backdrop"))
            .order(Order::Foreground)
            .fixed_pos(canvas.min)
            .show(ctx, |ui| {
                let (rect, response) = ui.allocate_exact_size(canvas.size(), Sense::click());
                ui.painter()
                    .rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(120));
                close = response.clicked();
            });
        let mut action = None;
        let mut submit = None;
        let id = Id::new(INPUT_ID);
        let area = Area::new(Id::new("assistant_dock"))
            .order(Order::Tooltip)
            .pivot(Align2::CENTER_TOP)
            .fixed_pos(pos2(canvas.center().x, canvas.top() + canvas.height() * 0.08))
            .show(ctx, |ui| {
                Frame::new()
                    .fill(theme::BG_ELEVATED())
                    .stroke(Stroke::new(1.2, theme::ACCENT().gamma_multiply(0.55)))
                    .corner_radius(CornerRadius::same(28))
                    .inner_margin(Margin::same(18))
                    .shadow(Shadow {
                        offset: [0, 30],
                        blur: 80,
                        spread: 0,
                        color: Color32::from_black_alpha(170),
                    })
                    .show(ui, |ui| {
                        ui.set_width(width - 36.0);
                        ui.set_height(height - 36.0);
                        // The prompt, large, with the mark and the mic.
                        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 56.0), Sense::hover());
                        let mark = Rect::from_center_size(pos2(row.left() + 24.0, row.center().y), vec2(40.0, 40.0));
                        icons::paint_mark(ui.painter(), mark);
                        let field = Rect::from_min_max(
                            pos2(row.left() + 58.0, row.top()),
                            pos2(row.right() - 56.0, row.bottom()),
                        );
                        submit = self.paint_pill_field(ui, field, id, "What should we work on?", false, false);
                        let mic = Rect::from_center_size(pos2(row.right() - 22.0, row.center().y), vec2(38.0, 38.0));
                        if super::super::mini::round_button(ui, mic, icons::Icon::Mic, "Talk to the assistant", false)
                            .clicked()
                        {
                            action = Some(Action::ToggleDictation);
                        }
                        ui.add_space(4.0);
                        ui.separator();
                        ui.add_space(8.0);
                        self.scope_strip(ui);
                        ui.add_space(8.0);
                        self.palette_view_row(ui);
                        ui.add_space(8.0);
                        let body = (ui.available_height() - 30.0).max(120.0);
                        self.conversation(ui, body, false);
                        ui.add_space(8.0);
                        ui.vertical_centered(|ui| {
                            ui.label(
                                egui::RichText::new(
                                    "Enter to send     Esc to put it away     Click a chip to look at one workspace",
                                )
                                .size(11.5)
                                .color(theme::FG_DIM()),
                            );
                        });
                    });
            });
        ctx.move_to_top(area.response.layer_id);
        self.assistant.summon.rect = Some(area.response.rect);
        self.render_scope_popup(ctx, &tiles);
        if close {
            self.assistant.summon.expanded = false;
        }
        if let Some(Action::Run(entry)) = submit {
            self.submit_summon(&entry);
        } else if let Some(key) = submit {
            self.apply_sheet_action(ctx, Some(key));
        }
        self.apply_sheet_action(ctx, action);
    }

    /// The choice of view, on the right under the chips.
    fn palette_view_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let (status, color) = self.feed_status();
            ui.label(egui::RichText::new(status).size(12.0).color(color));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.view_choice(ui);
            });
        });
    }
}
