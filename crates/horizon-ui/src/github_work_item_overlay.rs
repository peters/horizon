use std::time::Instant;

use egui::{
    Align, Button, Color32, Context, CornerRadius, Id, Layout, Margin, Order, Rect, RichText, Sense, Stroke,
    StrokeKind, UiBuilder, Vec2,
};
use horizon_core::GitHubWorkItemKind;

use crate::command_palette::render::paint_card;
use crate::theme;

const CARD_WIDTH: f32 = 560.0;
const INPUT_HEIGHT: f32 = 44.0;

pub struct GitHubWorkItemOverlay {
    kind: GitHubWorkItemKind,
    query: String,
    error: Option<String>,
    opened_at: Instant,
}

pub enum GitHubWorkItemOverlayAction {
    None,
    Cancelled,
    Submit(String),
}

impl GitHubWorkItemOverlay {
    pub fn new(kind: GitHubWorkItemKind) -> Self {
        Self {
            kind,
            query: String::new(),
            error: None,
            opened_at: Instant::now(),
        }
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    #[must_use]
    pub fn kind(&self) -> GitHubWorkItemKind {
        self.kind
    }

    pub fn show(&mut self, ctx: &Context, repo_hint: Option<&str>, resolving: bool) -> GitHubWorkItemOverlayAction {
        let screen = ctx.input(egui::InputState::viewport_rect);
        let card = Rect::from_center_size(screen.center(), Vec2::new(CARD_WIDTH, 220.0));
        let inner = card.shrink2(Vec2::new(18.0, 16.0));

        if self.show_backdrop(ctx, screen) {
            return GitHubWorkItemOverlayAction::Cancelled;
        }

        let mut action = GitHubWorkItemOverlayAction::None;
        egui::Area::new(Id::new("github_work_item_modal"))
            .fixed_pos(card.min)
            .constrain(true)
            .order(Order::Debug)
            .show(ctx, |ui| {
                paint_card(ui, card);
                ui.scope_builder(
                    UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)),
                    |ui| {
                        action = self.show_contents(ui, ctx, card.width(), repo_hint, resolving);
                    },
                );
            });

        action
    }

    fn show_backdrop(&self, ctx: &Context, screen_rect: Rect) -> bool {
        let mut cancelled = false;
        egui::Area::new(Id::new("github_work_item_backdrop"))
            .fixed_pos(screen_rect.min)
            .constrain(false)
            .order(Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                let (rect, response) = ui.allocate_exact_size(screen_rect.size(), Sense::click());
                ui.painter_at(rect)
                    .rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(140));
                if response.clicked() && self.opened_at.elapsed().as_millis() > 200 {
                    cancelled = true;
                }
            });
        cancelled
    }

    fn show_contents(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &Context,
        width: f32,
        repo_hint: Option<&str>,
        resolving: bool,
    ) -> GitHubWorkItemOverlayAction {
        ui.label(
            RichText::new(format!("Create Agent Workspace From {}", self.kind.label()))
                .size(15.0)
                .color(theme::FG)
                .strong(),
        );
        ui.add_space(6.0);
        let helper = repo_hint.map_or_else(
            || "Needs an active local GitHub repo in the focused workspace or panel.".to_string(),
            |repo| format!("Current repo: {repo}"),
        );
        ui.label(RichText::new(helper).size(11.0).color(theme::FG_DIM));
        ui.add_space(10.0);
        let placeholder = self.placeholder();
        let examples = self.examples();

        let input_rect = Rect::from_min_size(ui.cursor().min, Vec2::new(width - 36.0, INPUT_HEIGHT));
        ui.painter()
            .rect_filled(input_rect, CornerRadius::same(12), theme::BG_ELEVATED);
        ui.painter().rect_stroke(
            input_rect,
            CornerRadius::same(12),
            Stroke::new(1.0, theme::alpha(theme::ACCENT, 70)),
            StrokeKind::Inside,
        );

        let text_rect = input_rect.shrink2(Vec2::new(14.0, 6.0));
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(text_rect)
                .layout(Layout::left_to_right(Align::Center)),
        );
        let response = child.add(
            egui::TextEdit::singleline(&mut self.query)
                .font(egui::FontId::monospace(14.0))
                .text_color(theme::FG)
                .frame(false)
                .desired_width(text_rect.width())
                .hint_text(
                    RichText::new(placeholder)
                        .color(theme::FG_DIM)
                        .font(egui::FontId::monospace(11.0)),
                )
                .margin(Margin::ZERO),
        );
        if !response.has_focus() && self.opened_at.elapsed().as_millis() < 100 {
            response.request_focus();
        }
        if response.changed() {
            self.error = None;
        }

        ui.allocate_space(Vec2::new(width, INPUT_HEIGHT));
        ui.add_space(10.0);

        if let Some(error) = &self.error {
            ui.label(RichText::new(error).size(11.0).color(theme::PALETTE_RED));
            ui.add_space(6.0);
        }

        ui.label(RichText::new(examples).size(10.5).color(theme::FG_DIM));
        ui.add_space(12.0);

        let (pressed_enter, pressed_escape) = ctx.input(|input| {
            (
                input.key_pressed(egui::Key::Enter),
                input.key_pressed(egui::Key::Escape),
            )
        });
        if pressed_escape {
            return GitHubWorkItemOverlayAction::Cancelled;
        }

        let mut submitted = false;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!resolving, Button::new("Cancel").corner_radius(CornerRadius::same(8)))
                .clicked()
            {
                submitted = false;
                self.error = None;
            }
            let submit_label = if resolving { "Resolving..." } else { "Create Workspace" };
            if ui
                .add_enabled(
                    !resolving,
                    Button::new(submit_label).corner_radius(CornerRadius::same(8)),
                )
                .clicked()
            {
                submitted = true;
            }
        });

        if submitted || (pressed_enter && !resolving) {
            return GitHubWorkItemOverlayAction::Submit(self.query.trim().to_string());
        }

        GitHubWorkItemOverlayAction::None
    }

    fn placeholder(&self) -> &'static str {
        match self.kind {
            GitHubWorkItemKind::Issue => "#123 or https://github.com/org/repo/issues/123",
            GitHubWorkItemKind::PullRequest => "#69 or https://github.com/org/repo/pull/69",
            GitHubWorkItemKind::ReviewComment => {
                "discussion_r123456789 or https://github.com/org/repo/pull/69#discussion_r123456789"
            }
        }
    }

    fn examples(&self) -> &'static str {
        match self.kind {
            GitHubWorkItemKind::Issue => "Examples: `#123`, `peters/horizon#123`, full issue URL",
            GitHubWorkItemKind::PullRequest => "Examples: `#69`, `peters/horizon#69`, full PR URL",
            GitHubWorkItemKind::ReviewComment => "Examples: `discussion_r123456789`, `123456789`, full review URL",
        }
    }
}
