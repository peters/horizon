//! Design 2, the rail: a slim capsule on the left edge of the canvas, and a panel that opens from it.
//!
//! Mini: the orb, a mic and an arrow in a vertical capsule; news slides out to the right of it as cards.
//! Expanded: a panel docked to the left edge, full height, so the conversation sits beside the work
//! instead of over it.

use egui::{
    Align2, Area, Color32, Context, CornerRadius, Frame, Id, Margin, Order, Rect, Sense, Shadow, Stroke, StrokeKind,
    pos2, vec2,
};

use super::super::super::{icons, num};
use super::super::HorizonApp;
use super::super::deck::{CARD_HEIGHT, GAP};
use super::super::mini::{MiniAction, chevron_button, round_button};
use crate::theme;

const RAIL_WIDTH: f32 = 64.0;
const RAIL_HEIGHT: f32 = 204.0;
const TOAST_WIDTH: f32 = 440.0;
const PANEL_WIDTH: f32 = 480.0;
const EDGE: f32 = 16.0;

impl HorizonApp {
    pub(super) fn rail_mini(&mut self, ctx: &Context) {
        let canvas = self.canvas_rect(ctx);
        let toasts = self.deck_toasts();
        let stack = num::count(toasts.len()) * (CARD_HEIGHT + GAP);
        let width = RAIL_WIDTH + if toasts.is_empty() { 0.0 } else { 12.0 + TOAST_WIDTH };
        let height = RAIL_HEIGHT.max(stack);
        let mut actions: Vec<MiniAction> = Vec::new();
        let area = Area::new(Id::new("assistant_dock"))
            .order(Order::Tooltip)
            .pivot(Align2::LEFT_CENTER)
            .fixed_pos(pos2(canvas.left() + EDGE, canvas.center().y))
            .show(ctx, |ui| {
                let (whole, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
                let rail = Rect::from_min_size(
                    pos2(whole.left(), whole.center().y - RAIL_HEIGHT / 2.0),
                    vec2(RAIL_WIDTH, RAIL_HEIGHT),
                );
                self.paint_rail(ui, rail, &mut actions);
                if !toasts.is_empty() {
                    let cards = Rect::from_min_size(pos2(rail.right() + 12.0, whole.top()), vec2(TOAST_WIDTH, stack));
                    self.paint_toasts(ui, cards, &toasts, &mut actions);
                }
            });
        ctx.move_to_top(area.response.layer_id);
        self.assistant.summon.rect = Some(area.response.rect);
        for action in actions {
            self.apply_dock_action(ctx, &action);
        }
    }

    fn paint_rail(&mut self, ui: &mut egui::Ui, rail: Rect, actions: &mut Vec<MiniAction>) {
        let radius = CornerRadius::same(32);
        let agents = self.feed_agents();
        let now = num::seconds(ui);
        let listening = self
            .assistant
            .demo
            .as_ref()
            .is_some_and(super::super::demo::Demo::is_listening);
        let speaking = self.speaking_now();
        ui.painter().add(
            Shadow {
                offset: [6, 8],
                blur: 30,
                spread: 0,
                color: Color32::from_black_alpha(130),
            }
            .as_shape(rail, radius),
        );
        ui.painter().rect_filled(rail, radius, theme::BG_ELEVATED());
        ui.painter().rect_stroke(
            rail,
            radius,
            Stroke::new(1.2, theme::ACCENT().gamma_multiply(0.6)),
            StrokeKind::Inside,
        );
        let x = rail.center().x;
        let orb = pos2(x, rail.top() + 40.0);
        self.paint_orb(ui, orb, 23.0, &agents, now);
        if ui
            .interact(
                Rect::from_center_size(orb, vec2(52.0, 52.0)),
                Id::new("rail_orb"),
                Sense::click(),
            )
            .on_hover_text("Open the conversation")
            .clicked()
        {
            actions.push(MiniAction::Expand);
        }
        let mic = Rect::from_center_size(pos2(x, rail.top() + 106.0), vec2(38.0, 38.0));
        if round_button(
            ui,
            mic,
            icons::Icon::Mic,
            "Talk to the assistant",
            listening || speaking,
        )
        .clicked()
        {
            actions.push(MiniAction::Dictate);
        }
        let expand = Rect::from_center_size(pos2(x, rail.top() + 160.0), vec2(34.0, 34.0));
        if chevron_button(ui, expand).clicked() {
            actions.push(MiniAction::Expand);
        }
    }

    pub(super) fn rail_panel(&mut self, ctx: &Context) {
        let canvas = self.canvas_rect(ctx);
        let width = PANEL_WIDTH.min(canvas.width() - 2.0 * EDGE);
        let height = canvas.height() - 2.0 * EDGE;
        let tiles = self.dock_tiles();
        let mut action = None;
        let area = Area::new(Id::new("assistant_dock"))
            .order(Order::Tooltip)
            .pivot(Align2::LEFT_TOP)
            .fixed_pos(pos2(canvas.left() + EDGE, canvas.top() + EDGE))
            .show(ctx, |ui| {
                Frame::new()
                    .fill(theme::BG_ELEVATED())
                    .stroke(Stroke::new(1.2, theme::ACCENT().gamma_multiply(0.5)))
                    .corner_radius(CornerRadius::same(24))
                    .inner_margin(Margin::same(16))
                    .shadow(Shadow {
                        offset: [10, 12],
                        blur: 50,
                        spread: 0,
                        color: Color32::from_black_alpha(150),
                    })
                    .show(ui, |ui| {
                        ui.set_width(width - 32.0);
                        ui.set_height(height - 32.0);
                        self.dock_header(ui, &mut action);
                        ui.add_space(10.0);
                        self.scope_strip(ui);
                        ui.add_space(10.0);
                        let body = (ui.available_height() - 62.0 - 8.0).max(120.0);
                        self.conversation(ui, body, false);
                        ui.add_space(8.0);
                        self.desk_prompt(ui, &mut action);
                    });
            });
        ctx.move_to_top(area.response.layer_id);
        self.assistant.summon.rect = Some(area.response.rect);
        self.render_scope_popup(ctx, &tiles);
        self.apply_sheet_action(ctx, action);
    }
}
