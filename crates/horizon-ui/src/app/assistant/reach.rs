//! The "In reach" strip: the other agents of the assistant's workspace and what
//! each is doing. Clicking one brings it into view on the canvas.

use egui::{Color32, CornerRadius, FontId, Margin, Response, RichText, Sense, Stroke, StrokeKind, Ui, vec2};
use horizon_core::PanelId;
use horizon_core::browser::manifest::agent_panels::{AgentPanel, AgentState};

use super::HorizonApp;
use crate::theme;

/// Reading an agent's state locks its terminal, so the strip refreshes a few times a second.
const REFRESH_EVERY: std::time::Duration = std::time::Duration::from_millis(500);

impl HorizonApp {
    fn reach_agents(&mut self, ctx: &egui::Context, assistant: PanelId) -> Vec<AgentPanel> {
        let now = std::time::Instant::now();
        if let Some((read_at, agents)) = &self.assistant.reach_cache
            && now.duration_since(*read_at) < REFRESH_EVERY
        {
            ctx.request_repaint_after(REFRESH_EVERY);
            return agents.clone();
        }
        let agents: Vec<_> = self
            .board
            .agent_panels_in(&self.assistant_reach(), assistant)
            .into_iter()
            .filter(|agent| !agent.is_caller)
            .collect();
        self.assistant.reach_cache = Some((now, agents.clone()));
        ctx.request_repaint_after(REFRESH_EVERY);
        agents
    }

    pub(super) fn render_reach_strip(&mut self, ui: &mut Ui) {
        let Some(assistant) = self.board.assistant_panel() else {
            return;
        };
        let agents = self.reach_agents(ui.ctx(), assistant);
        if agents.is_empty() {
            return;
        }
        let mut reveal = None;
        egui::Frame::new()
            .inner_margin(Margin::symmetric(14, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("In reach").size(11.5).color(theme::FG_DIM()));
                    for agent in &agents {
                        let (label, color) = state_style(agent.state);
                        let response = pill(ui, &agent.title, color).on_hover_text(format!(
                            "{} - {label}",
                            agent.directory.as_deref().unwrap_or(&agent.kind)
                        ));
                        if response.clicked() {
                            reveal = Some(agent.panel_id.clone());
                        }
                    }
                });
            });
        ui.painter().hline(
            ui.max_rect().x_range(),
            ui.cursor().top(),
            Stroke::new(1.0, theme::BORDER_SUBTLE()),
        );
        if let Some(local_id) = reveal
            && let Some(panel_id) = self.board.panel_id_by_local_id(&local_id)
        {
            self.reveal_selected_panel(ui.ctx(), panel_id);
        }
    }
}

fn state_style(state: AgentState) -> (&'static str, Color32) {
    match state {
        AgentState::Starting => ("starting", theme::FG_DIM()),
        AgentState::Working => ("working", theme::ACCENT()),
        AgentState::Idle => ("idle", theme::PALETTE_GREEN()),
        AgentState::NeedsInput => ("needs input", theme::PALETTE_YELLOW()),
        AgentState::Exited => ("exited", theme::PALETTE_RED()),
    }
}

/// A rounded chip with a state dot, sized from its measured text.
fn pill(ui: &mut Ui, text: &str, dot: Color32) -> Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), FontId::proportional(11.5), theme::FG_SOFT());
    let size = galley.size() + vec2(28.0, 8.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(99),
        theme::PANEL_BG_ALT(),
        Stroke::new(1.0, theme::BORDER_SUBTLE()),
        StrokeKind::Inside,
    );
    painter.circle_filled(rect.left_center() + vec2(11.0, 0.0), 3.0, dot);
    painter.galley(rect.left_top() + vec2(20.0, 4.0), galley, theme::FG_SOFT());
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
