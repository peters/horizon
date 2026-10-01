//! Spaces and threads: the sessions the assistant has run, grouped by the
//! workspace they ran in, and a bar to switch between them. Switching resumes
//! the chosen session with the hosted agent's own resume flag.

use std::time::{Duration, Instant};

use egui::{Align2, Color32, CornerRadius, Frame, Id, Key, Margin, Order, RichText, Sense, Shadow, Stroke, Ui, vec2};
use horizon_core::assistant::Thread;
use horizon_core::browser::manifest::now_millis;

use super::HorizonApp;
use crate::theme;

const SYNC_EVERY: Duration = Duration::from_secs(1);
const MENU_WIDTH: f32 = 340.0;

enum MenuAction {
    Switch(String),
    Forget(String),
    New,
}

impl HorizonApp {
    /// Records the running agent's session as a thread and keeps titles fresh.
    pub(super) fn sync_assistant_thread(&mut self) {
        let now = Instant::now();
        if self
            .assistant
            .last_thread_sync
            .is_some_and(|last| now.duration_since(last) < SYNC_EVERY)
        {
            return;
        }
        self.assistant.last_thread_sync = Some(now);
        let Some(panel) = self
            .board
            .assistant_panel()
            .and_then(|panel_id| self.board.panel(panel_id))
        else {
            self.assistant.active_session = None;
            return;
        };
        let Some(binding) = panel.session_binding.clone() else {
            self.assistant.active_session = None;
            return;
        };
        let space = self
            .board
            .workspace(panel.workspace_id)
            .map_or_else(String::new, |workspace| workspace.name.clone());
        let cwd = binding
            .cwd
            .clone()
            .or_else(|| panel.launch_cwd.as_ref().map(|path| path.display().to_string()));
        let title = self.known_thread_title(binding.kind, &binding.session_id);
        let thread = Thread {
            session_id: binding.session_id.clone(),
            agent: binding.kind,
            title,
            space,
            cwd,
            updated_at: now_millis(),
        };
        self.assistant.active_session = Some(binding.session_id);
        if self.assistant.threads.upsert(thread) {
            self.save_threads();
        }
    }

    /// The agent's own title for a session, or an empty string while it has none.
    fn known_thread_title(&self, kind: horizon_core::PanelKind, session_id: &str) -> String {
        if let Some(thread) = self.assistant.threads.get(session_id)
            && !thread.title.is_empty()
        {
            return thread.title.clone();
        }
        self.session_catalog
            .recent_for(kind, None)
            .into_iter()
            .find(|record| record.session_id == session_id)
            .and_then(|record| record.label)
            .map(|label| label.trim().to_string())
            .unwrap_or_default()
    }

    fn save_threads(&mut self) {
        if let Err(error) = self.assistant.threads.save(&self.assistant.home) {
            tracing::warn!(%error, "could not save the assistant threads");
        }
    }

    /// Resumes a remembered session in the drawer.
    pub(super) fn switch_assistant_thread(&mut self, session_id: &str) {
        self.assistant.thread_menu_open = false;
        if self.assistant.active_session.as_deref() == Some(session_id) {
            return;
        }
        let Some(thread) = self.assistant.threads.get(session_id).cloned() else {
            return;
        };
        if thread.agent != self.assistant.settings.agent {
            self.assistant.settings.agent = thread.agent;
            self.assistant.draft = self.assistant.settings;
            if let Err(error) = self.assistant.settings.save(&self.assistant.home) {
                self.assistant.notice = Some(format!("Could not save the assistant settings: {error}"));
            }
        }
        self.forget_untitled_active_thread();
        if self.assistant.threads.touch(session_id, now_millis()) {
            self.save_threads();
        }
        self.assistant.resume = Some(thread);
        self.restart_assistant();
    }

    /// A thread that never got a first message has nothing worth coming back to.
    fn forget_untitled_active_thread(&mut self) {
        let Some(session) = self.assistant.active_session.clone() else {
            return;
        };
        if self
            .assistant
            .threads
            .get(&session)
            .is_some_and(|thread| thread.title.is_empty())
            && self.assistant.threads.forget(&session)
        {
            self.save_threads();
        }
    }

    /// Starts a fresh session. The old one stays in the thread list.
    pub(super) fn new_assistant_thread(&mut self) {
        self.forget_untitled_active_thread();
        self.assistant.resume = None;
        self.assistant.active_session = None;
        self.assistant.thread_menu_open = false;
        self.restart_assistant();
    }

    pub(super) fn render_thread_bar(&mut self, ui: &mut Ui) {
        let active = self
            .assistant
            .active_session
            .as_deref()
            .and_then(|id| self.assistant.threads.get(id));
        let title = active
            .map(|thread| thread.title.as_str())
            .filter(|title| !title.is_empty())
            .unwrap_or("New thread")
            .to_string();
        let space = active
            .map(|thread| thread.space.clone())
            .filter(|space| !space.is_empty());
        Frame::new().inner_margin(Margin::symmetric(14, 7)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if let Some(space) = space {
                    ui.label(RichText::new(space).size(11.5).color(theme::FG_DIM()));
                    ui.label(RichText::new("/").size(11.5).color(theme::BORDER_STRONG()));
                }
                let label = ui
                    .add(
                        egui::Label::new(RichText::new(shortened(&title)).size(12.5).color(theme::FG()))
                            .sense(Sense::click()),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                let (arrow, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                paint_chevron(ui, arrow.center(), theme::FG_DIM());
                self.assistant.thread_anchor = Some(label.rect);
                if label.clicked() {
                    self.assistant.thread_menu_open = !self.assistant.thread_menu_open;
                }
            });
        });
        ui.painter().hline(
            ui.max_rect().x_range(),
            ui.cursor().top(),
            Stroke::new(1.0, theme::BORDER_SUBTLE()),
        );
    }

    pub(super) fn render_thread_menu(&mut self, ctx: &egui::Context) {
        if !self.assistant.thread_menu_open {
            return;
        }
        let Some(anchor) = self.assistant.thread_anchor else {
            return;
        };
        let groups: Vec<(String, Vec<Thread>)> = self
            .assistant
            .threads
            .by_space()
            .into_iter()
            .map(|(space, threads)| (space.to_string(), threads.into_iter().cloned().collect()))
            .collect();
        let mut action = None;
        let area = egui::Area::new(Id::new("assistant_thread_menu"))
            .order(Order::Foreground)
            .pivot(Align2::LEFT_TOP)
            .fixed_pos(anchor.left_bottom() + vec2(-8.0, 8.0))
            .show(ctx, |ui| {
                Frame::new()
                    .fill(theme::BG_ELEVATED())
                    .stroke(Stroke::new(1.0, theme::BORDER_STRONG()))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(Margin::same(12))
                    .shadow(Shadow {
                        offset: [0, 12],
                        blur: 36,
                        spread: 2,
                        color: Color32::from_black_alpha(120),
                    })
                    .show(ui, |ui| {
                        ui.set_width(MENU_WIDTH);
                        action = self.thread_menu_body(ui, &groups);
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
            self.assistant.thread_menu_open = false;
        }
        match action {
            Some(MenuAction::Switch(id)) => self.switch_assistant_thread(&id),
            Some(MenuAction::Forget(id)) => {
                if self.assistant.threads.forget(&id) {
                    self.save_threads();
                }
            }
            Some(MenuAction::New) => self.new_assistant_thread(),
            None => {}
        }
    }

    fn thread_menu_body(&self, ui: &mut Ui, groups: &[(String, Vec<Thread>)]) -> Option<MenuAction> {
        let mut action = None;
        if groups.is_empty() {
            ui.label(
                RichText::new("Threads show up here once the assistant has run.")
                    .size(11.5)
                    .color(theme::FG_DIM()),
            );
        }
        for (space, threads) in groups {
            ui.label(
                RichText::new(
                    if space.is_empty() {
                        "NO WORKSPACE"
                    } else {
                        space.as_str()
                    }
                    .to_uppercase(),
                )
                .size(10.5)
                .extra_letter_spacing(0.9)
                .color(theme::FG_DIM()),
            );
            ui.add_space(3.0);
            for thread in threads {
                let selected = self.assistant.active_session.as_deref() == Some(thread.session_id.as_str());
                ui.horizontal(|ui| {
                    let name = if thread.title.is_empty() {
                        "Untitled thread"
                    } else {
                        thread.title.as_str()
                    };
                    let color = if selected { theme::FG() } else { theme::FG_SOFT() };
                    let agent = horizon_core::agent_definition(thread.agent).map_or("", |agent| agent.display_name);
                    let button = egui::Button::new(RichText::new(shortened(name)).size(12.5).color(color))
                        .right_text(RichText::new(agent).size(11.0).color(theme::FG_DIM()))
                        .selected(selected)
                        .min_size(vec2(ui.available_width() - 30.0, 30.0));
                    if ui.add(button).clicked() {
                        action = Some(MenuAction::Switch(thread.session_id.clone()));
                    }
                    if ui
                        .small_button("x")
                        .on_hover_text("Forget this thread. The agent keeps its own history.")
                        .clicked()
                    {
                        action = Some(MenuAction::Forget(thread.session_id.clone()));
                    }
                });
            }
            ui.add_space(8.0);
        }
        if ui.button("+  New thread").clicked() {
            action = Some(MenuAction::New);
        }
        action
    }
}

fn shortened(text: &str) -> String {
    const MAX: usize = 38;
    if text.chars().count() <= MAX {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(MAX - 1).collect();
    cut.push('…');
    cut
}

fn paint_chevron(ui: &Ui, center: egui::Pos2, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    let painter = ui.painter();
    painter.line_segment([center + vec2(-3.5, -1.5), center + vec2(0.0, 2.0)], stroke);
    painter.line_segment([center + vec2(0.0, 2.0), center + vec2(3.5, -1.5)], stroke);
}

#[cfg(test)]
mod tests;
