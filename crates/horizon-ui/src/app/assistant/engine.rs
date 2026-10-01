//! Engine popup: which agent answers and how it signs in, like choosing a panel kind.

use egui::{
    Align2, Button, Color32, Context, CornerRadius, Frame, Id, Key, Margin, Order, RichText, Shadow, Stroke, TextEdit,
    Ui, pos2, vec2,
};
use horizon_core::assistant::{
    ASSISTANT_AGENTS, AssistantAuth, api_key_binding, has_api_key, remove_api_key, save_api_key,
};
use horizon_core::{HorizonHome, PanelKind, agent_definition};

use super::HorizonApp;
use crate::theme;

const WIDTH: f32 = 340.0;

impl HorizonApp {
    pub(super) fn render_assistant_engine_popup(&mut self, ctx: &Context) {
        if !self.assistant.engine_open {
            return;
        }
        let anchor = self
            .assistant
            .engine_anchor
            .unwrap_or_else(|| egui::Rect::from_min_size(pos2(200.0, 60.0), vec2(60.0, 28.0)));
        let area = egui::Area::new(Id::new("assistant_engine_popup"))
            .order(Order::Foreground)
            .pivot(Align2::RIGHT_TOP)
            .fixed_pos(anchor.right_bottom() + vec2(0.0, 8.0))
            .show(ctx, |ui| {
                Frame::new()
                    .fill(theme::BG_ELEVATED())
                    .stroke(Stroke::new(1.0, theme::BORDER_STRONG()))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(Margin::same(14))
                    .shadow(Shadow {
                        offset: [0, 12],
                        blur: 36,
                        spread: 2,
                        color: Color32::from_black_alpha(120),
                    })
                    .show(ui, |ui| {
                        ui.set_width(WIDTH);
                        self.engine_popup_body(ui);
                    });
            });
        let rect = area.response.rect;
        let outside = ctx.input(|input| {
            input.pointer.any_pressed()
                && input
                    .pointer
                    .interact_pos()
                    .is_some_and(|pos| !rect.contains(pos) && !anchor.contains(pos))
        });
        if outside || ctx.input(|input| input.key_pressed(Key::Escape)) {
            self.assistant.engine_open = false;
        }
    }

    fn engine_popup_body(&mut self, ui: &mut Ui) {
        section_label(ui, "Agent");
        ui.add_space(4.0);
        for kind in ASSISTANT_AGENTS {
            let Some(definition) = agent_definition(kind) else {
                continue;
            };
            let selected = self.assistant.draft.agent == kind;
            let color = if selected { theme::FG() } else { theme::FG_SOFT() };
            let detail = if matches!(kind, PanelKind::Claude | PanelKind::Codex | PanelKind::Grok) {
                "Horizon tools"
            } else {
                "terminal only"
            };
            let button = Button::new(RichText::new(definition.display_name).size(13.0).color(color))
                .right_text(RichText::new(detail).size(11.0).color(theme::FG_DIM()))
                .selected(selected)
                .min_size(vec2(ui.available_width(), 30.0));
            if ui.add(button).clicked() {
                self.assistant.draft.agent = kind;
                self.assistant.key_input.clear();
            }
        }

        ui.add_space(12.0);
        section_label(ui, "Sign in with");
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let half = (ui.available_width() - ui.spacing().item_spacing.x) / 2.0;
            for (auth, label) in [
                (AssistantAuth::Subscription, "Subscription"),
                (AssistantAuth::ApiKey, "API key"),
            ] {
                let selected = self.assistant.draft.auth == auth;
                let color = if selected { theme::FG() } else { theme::FG_SOFT() };
                let button = Button::new(RichText::new(label).size(12.5).color(color))
                    .selected(selected)
                    .min_size(vec2(half, 32.0));
                if ui.add(button).clicked() {
                    self.assistant.draft.auth = auth;
                }
            }
        });
        ui.add_space(10.0);
        match self.assistant.draft.auth {
            AssistantAuth::Subscription => self.subscription_note(ui),
            AssistantAuth::ApiKey => self.api_key_section(ui),
        }

        ui.add_space(12.0);
        let home = HorizonHome::resolve();
        let readiness = self.assistant.draft.launch_readiness(&home);
        let changed = self.assistant.draft != self.assistant.settings;
        ui.horizontal(|ui| {
            let apply = ui.add_enabled(
                changed && readiness.is_ok(),
                Button::new(
                    RichText::new("Restart with this engine")
                        .size(12.5)
                        .strong()
                        .color(Color32::from_rgb(7, 16, 31)),
                )
                .fill(theme::ACCENT())
                .corner_radius(CornerRadius::same(8))
                .min_size(vec2(0.0, 32.0)),
            );
            if apply.clicked() {
                self.apply_assistant_engine();
                self.assistant.engine_open = false;
            }
            if ui.button("Close").clicked() {
                self.assistant.engine_open = false;
            }
        });
        if let Err(reason) = readiness {
            ui.add_space(6.0);
            ui.label(RichText::new(reason).size(11.5).color(theme::PALETTE_YELLOW()));
        }
    }

    fn subscription_note(&self, ui: &mut Ui) {
        let name = agent_definition(self.assistant.draft.agent).map_or("agent", |agent| agent.display_name);
        ui.label(
            RichText::new(format!(
                "Uses the account already signed in to the {name} CLI. Horizon stores no key. \
                 An API key already set in your environment may take precedence."
            ))
            .size(11.5)
            .color(theme::FG_DIM()),
        );
    }

    fn api_key_section(&mut self, ui: &mut Ui) {
        let kind = self.assistant.draft.agent;
        let Some((variable, _)) = api_key_binding(kind) else {
            ui.label(
                RichText::new("This agent has no API key mode here. Use its own sign-in.")
                    .size(11.5)
                    .color(theme::FG_DIM()),
            );
            return;
        };
        let home = HorizonHome::resolve();
        ui.label(
            RichText::new(format!("Passed to the agent as {variable}, kept in a private file."))
                .size(11.5)
                .color(theme::FG_DIM()),
        );
        ui.add_space(6.0);
        if has_api_key(&home, kind) {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Key saved").size(12.5).color(theme::PALETTE_GREEN()));
                if ui.button("Remove").clicked()
                    && let Err(error) = remove_api_key(&home, kind)
                {
                    self.assistant.notice = Some(format!("Could not remove the key: {error}"));
                }
            });
            return;
        }
        ui.add(
            TextEdit::singleline(&mut *self.assistant.key_input)
                .password(true)
                .hint_text("Paste your key")
                .desired_width(f32::INFINITY),
        );
        ui.add_space(6.0);
        let enabled = !self.assistant.key_input.trim().is_empty();
        if ui.add_enabled(enabled, Button::new("Save key")).clicked() {
            match save_api_key(&home, kind, &self.assistant.key_input) {
                Ok(()) => self.assistant.key_input.clear(),
                Err(error) => self.assistant.notice = Some(error.to_string()),
            }
        }
    }
}

fn section_label(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text.to_uppercase())
            .size(10.5)
            .extra_letter_spacing(0.9)
            .color(theme::FG_DIM()),
    );
}
