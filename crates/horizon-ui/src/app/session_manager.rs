use egui::{Align, Align2, Color32, Context, CornerRadius, Id, Layout, Margin, RichText, Stroke, Vec2};
use horizon_core::{ResolvedSession, SessionSummary};

use crate::theme;

use super::HorizonApp;
use super::util::{chrome_button, format_relative_time, primary_button};

const SESSION_MANAGER_WIDTH: f32 = 780.0;
const SESSION_MANAGER_MAX_HEIGHT: f32 = 520.0;

#[derive(Clone, Debug)]
pub(super) struct RuntimeSessionManagerState {
    sessions: Vec<SessionSummary>,
    selected_session_id: Option<String>,
    error: Option<String>,
    confirm_clear_others: bool,
}

impl RuntimeSessionManagerState {
    fn new(
        sessions: Vec<SessionSummary>,
        preferred_selection: Option<String>,
        current_session_id: Option<&str>,
        error: Option<String>,
    ) -> Self {
        let selected_session_id = preferred_selection
            .filter(|session_id| sessions.iter().any(|session| session.session_id == *session_id))
            .or_else(|| {
                current_session_id
                    .filter(|session_id| sessions.iter().any(|session| session.session_id == *session_id))
                    .map(ToOwned::to_owned)
            })
            .or_else(|| sessions.first().map(|session| session.session_id.clone()));

        Self {
            sessions,
            selected_session_id,
            error,
            confirm_clear_others: false,
        }
    }

    fn selected_session(&self) -> Option<&SessionSummary> {
        let selected_session_id = self.selected_session_id.as_deref()?;
        self.sessions
            .iter()
            .find(|session| session.session_id == selected_session_id)
    }

    fn removable_session_ids(&self, current_session_id: Option<&str>) -> Vec<String> {
        self.sessions
            .iter()
            .filter(|session| Some(session.session_id.as_str()) != current_session_id && !session.is_live)
            .map(|session| session.session_id.clone())
            .collect()
    }
}

enum SessionManagerAction {
    None,
    Close,
    CreateNewSession,
    SwitchTo(String),
    Remove(String),
    RemoveAll(Vec<String>),
}

struct SessionManagerViewState {
    removable_session_ids: Vec<String>,
    selected_session: Option<SessionSummary>,
    can_switch: bool,
    can_remove_selected: bool,
    remove_all_label: &'static str,
}

impl SessionManagerViewState {
    fn new(state: &RuntimeSessionManagerState, current_session_id: Option<&str>) -> Self {
        let removable_session_ids = state.removable_session_ids(current_session_id);
        let selected_session = state.selected_session().cloned();
        let can_switch = selected_session
            .as_ref()
            .is_some_and(|session| Some(session.session_id.as_str()) != current_session_id && !session.is_live);
        let can_remove_selected = selected_session
            .as_ref()
            .is_some_and(|session| Some(session.session_id.as_str()) != current_session_id && !session.is_live);
        let remove_all_label = if current_session_id.is_some() {
            "Remove Other Sessions"
        } else {
            "Remove All Saved Sessions"
        };

        Self {
            removable_session_ids,
            selected_session,
            can_switch,
            can_remove_selected,
            remove_all_label,
        }
    }

    fn has_removable_sessions(&self) -> bool {
        !self.removable_session_ids.is_empty()
    }
}

impl HorizonApp {
    pub(super) fn toggle_session_manager(&mut self) {
        if self.session_manager.is_some() {
            self.session_manager = None;
            return;
        }

        self.command_palette = None;
        self.remote_hosts_overlay = None;
        self.reload_session_manager(None, None);
    }

    pub(super) fn render_session_manager(&mut self, ctx: &Context) {
        let current_session_id = self.active_persistent_session_id().map(ToOwned::to_owned);
        let action = {
            let Some(state) = self.session_manager.as_mut() else {
                return;
            };
            render_backdrop(ctx);
            render_session_manager_window(ctx, state, current_session_id.as_deref())
        };

        match action {
            SessionManagerAction::None => {}
            SessionManagerAction::Close => self.session_manager = None,
            SessionManagerAction::CreateNewSession => {
                match self.session_store.create_new_session(&self.template_config) {
                    Ok(session) => self.activate_runtime_session(ctx, &session),
                    Err(error) => self.set_session_manager_error(format!("Failed to create session: {error}")),
                }
            }
            SessionManagerAction::SwitchTo(session_id) => match self.session_store.resume_session(&session_id) {
                Ok(session) => self.activate_runtime_session(ctx, &session),
                Err(error) => self.set_session_manager_error(format!("Failed to switch session: {error}")),
            },
            SessionManagerAction::Remove(session_id) => match self.session_store.delete_session(&session_id) {
                Ok(()) => self.reload_session_manager(None, None),
                Err(error) => self.set_session_manager_error(format!("Failed to remove session: {error}")),
            },
            SessionManagerAction::RemoveAll(session_ids) => {
                for session_id in session_ids {
                    if let Err(error) = self.session_store.delete_session(&session_id) {
                        self.reload_session_manager(None, Some(format!("Failed to remove sessions: {error}")));
                        return;
                    }
                }
                self.reload_session_manager(None, None);
            }
        }
    }

    fn reload_session_manager(&mut self, preferred_selection: Option<String>, error: Option<String>) {
        let current_session_id = self.active_persistent_session_id().map(ToOwned::to_owned);
        let selected_session_id = preferred_selection.or_else(|| self.session_manager_selected_id());
        let state = match self.session_store.list_profile_sessions() {
            Ok(sessions) => {
                RuntimeSessionManagerState::new(sessions, selected_session_id, current_session_id.as_deref(), error)
            }
            Err(load_error) => RuntimeSessionManagerState::new(
                Vec::new(),
                None,
                current_session_id.as_deref(),
                Some(format!("Failed to load sessions: {load_error}")),
            ),
        };
        self.session_manager = Some(state);
    }

    fn session_manager_selected_id(&self) -> Option<String> {
        self.session_manager
            .as_ref()
            .and_then(|state| state.selected_session_id.clone())
    }

    fn set_session_manager_error(&mut self, error: String) {
        if let Some(state) = self.session_manager.as_mut() {
            state.error = Some(error);
            state.confirm_clear_others = false;
        }
    }

    fn active_persistent_session_id(&self) -> Option<&str> {
        self.active_session
            .as_ref()
            .filter(|session| session.persistent)
            .map(|session| session.session_id.as_str())
    }

    fn activate_runtime_session(&mut self, ctx: &Context, session: &ResolvedSession) {
        self.prepare_session_switch();
        self.activate_persistent_session(session);
        self.restore_window_viewport(ctx);
        self.session_manager = None;
    }

    fn prepare_session_switch(&mut self) {
        self.auto_save_runtime_state();
        self.board.shutdown_terminal_panels();
        self.git_watchers.clear();
        self.release_active_session_lease();
        self.active_session = None;
        self.last_terminal_output_at = None;
        self.fullscreen_panel = None;
        self.clear_workspace_rename();
        self.clear_panel_rename();
        self.command_palette = None;
        self.search_overlay = None;
        self.remote_hosts_overlay = None;
        self.dir_picker = None;
        self.pending_preset_pick = None;
        self.ssh_upload_flow = None;
        self.panels_to_close.clear();
        self.panels_to_restart.clear();
        self.workspace_assignments.clear();
        self.workspace_creates.clear();
        self.pending_session_rebinds.clear();
        self.panel_screen_rects.clear();
        self.terminal_body_screen_rects.clear();
        self.panel_screen_order.clear();
        self.workspace_screen_rects.clear();
        self.file_drop_highlight = None;
        self.file_hover_positions.clear();
    }
}

fn render_backdrop(ctx: &Context) {
    let screen_rect = ctx.input(egui::InputState::viewport_rect);
    egui::Area::new(Id::new("session_manager_backdrop"))
        .fixed_pos(screen_rect.min)
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(screen_rect.size(), egui::Sense::hover());
            ui.painter_at(rect)
                .rect_filled(rect, 0.0, Color32::from_black_alpha(156));
        });
}

fn render_session_manager_window(
    ctx: &Context,
    state: &mut RuntimeSessionManagerState,
    current_session_id: Option<&str>,
) -> SessionManagerAction {
    let mut action = SessionManagerAction::None;
    let view_state = SessionManagerViewState::new(state, current_session_id);

    egui::Window::new("session_manager_modal")
        .id(Id::new("session_manager_modal"))
        .title_bar(false)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .collapsible(false)
        .resizable(false)
        .fixed_size(Vec2::new(SESSION_MANAGER_WIDTH, 0.0))
        .order(egui::Order::Tooltip)
        .frame(
            egui::Frame::NONE
                .fill(theme::PANEL_BG)
                .stroke(Stroke::new(1.0, theme::BORDER_STRONG))
                .corner_radius(CornerRadius::same(16))
                .shadow(egui::Shadow {
                    offset: [0, 12],
                    blur: 42,
                    spread: 4,
                    color: Color32::from_black_alpha(144),
                }),
        )
        .show(ctx, |ui| {
            ui.set_max_height(SESSION_MANAGER_MAX_HEIGHT);
            render_header(ui, &mut action);
            render_session_manager_content(ui, state, current_session_id, &view_state, &mut action);
        });

    action
}

fn render_session_manager_content(
    ui: &mut egui::Ui,
    state: &mut RuntimeSessionManagerState,
    current_session_id: Option<&str>,
    view_state: &SessionManagerViewState,
    action: &mut SessionManagerAction,
) {
    egui::Frame::NONE.inner_margin(Margin::symmetric(24, 0)).show(ui, |ui| {
        ui.add_space(18.0);
        ui.label(
            RichText::new("Switch between saved sessions, create a fresh one, or prune inactive sessions.")
                .size(12.5)
                .color(theme::FG_SOFT),
        );
        ui.add_space(14.0);

        render_session_list(ui, state, current_session_id);
        render_session_error(ui, state.error.as_deref());
        render_session_hint(ui, view_state.selected_session.as_ref(), current_session_id);
        render_session_actions(ui, state, view_state, action);
        ui.add_space(20.0);
    });
}

fn render_session_list(ui: &mut egui::Ui, state: &mut RuntimeSessionManagerState, current_session_id: Option<&str>) {
    if state.sessions.is_empty() {
        render_empty_state(ui);
        return;
    }

    egui::ScrollArea::vertical()
        .max_height(300.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for session in &state.sessions {
                if render_session_card(
                    ui,
                    session,
                    state.selected_session_id.as_deref() == Some(session.session_id.as_str()),
                    current_session_id,
                ) {
                    state.selected_session_id = Some(session.session_id.clone());
                    state.confirm_clear_others = false;
                    state.error = None;
                }
                ui.add_space(10.0);
            }
        });
}

fn render_session_error(ui: &mut egui::Ui, error: Option<&str>) {
    if let Some(error) = error {
        ui.add_space(8.0);
        ui.label(RichText::new(error).size(11.5).color(theme::PALETTE_RED));
    }
}

fn render_session_hint(ui: &mut egui::Ui, selected_session: Option<&SessionSummary>, current_session_id: Option<&str>) {
    ui.add_space(10.0);
    let hint = if selected_session.is_some_and(|session| session.is_live)
        && selected_session.is_some_and(|session| Some(session.session_id.as_str()) != current_session_id)
    {
        "Live sessions in another Horizon instance cannot be switched or removed here."
    } else {
        "The current session stays on disk, but it cannot be removed until you switch away from it."
    };
    ui.label(RichText::new(hint).size(11.0).color(theme::FG_DIM));
    ui.add_space(16.0);
}

fn render_session_actions(
    ui: &mut egui::Ui,
    state: &mut RuntimeSessionManagerState,
    view_state: &SessionManagerViewState,
    action: &mut SessionManagerAction,
) {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if ui.add(primary_button("Open New Session")).clicked() {
            *action = SessionManagerAction::CreateNewSession;
        }

        if ui
            .add_enabled(
                view_state.can_switch,
                primary_button("Switch Selected").min_size(Vec2::new(128.0, 0.0)),
            )
            .clicked()
            && let Some(session) = view_state.selected_session.as_ref()
        {
            *action = SessionManagerAction::SwitchTo(session.session_id.clone());
        }

        if ui
            .add_enabled(
                view_state.can_remove_selected,
                chrome_button("Remove Selected").min_size(Vec2::new(118.0, 0.0)),
            )
            .clicked()
            && let Some(session) = view_state.selected_session.as_ref()
        {
            *action = SessionManagerAction::Remove(session.session_id.clone());
        }

        render_remove_all_actions(ui, state, view_state, action);

        if ui.add(chrome_button("Close")).clicked() {
            *action = SessionManagerAction::Close;
        }
    });
}

fn render_remove_all_actions(
    ui: &mut egui::Ui,
    state: &mut RuntimeSessionManagerState,
    view_state: &SessionManagerViewState,
    action: &mut SessionManagerAction,
) {
    if state.confirm_clear_others {
        if ui
            .add_enabled(
                view_state.has_removable_sessions(),
                chrome_button("Confirm Remove All").min_size(Vec2::new(146.0, 0.0)),
            )
            .clicked()
        {
            *action = SessionManagerAction::RemoveAll(view_state.removable_session_ids.clone());
        }
        if ui.add(chrome_button("Cancel")).clicked() {
            state.confirm_clear_others = false;
        }
        return;
    }

    if ui
        .add_enabled(
            view_state.has_removable_sessions(),
            chrome_button(view_state.remove_all_label).min_size(Vec2::new(156.0, 0.0)),
        )
        .clicked()
    {
        state.confirm_clear_others = true;
        state.error = None;
    }
}

fn render_header(ui: &mut egui::Ui, action: &mut SessionManagerAction) {
    egui::Frame::NONE
        .fill(theme::TITLEBAR_BG)
        .corner_radius(CornerRadius {
            nw: 16,
            ne: 16,
            sw: 0,
            se: 0,
        })
        .inner_margin(Margin::symmetric(24, 18))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.label(RichText::new("Sessions").size(18.0).strong().color(theme::FG));
                    ui.label(
                        RichText::new("Manage Horizon sessions without restarting the app.")
                            .size(11.5)
                            .color(theme::FG_DIM),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(chrome_button("Close")).clicked() {
                        *action = SessionManagerAction::Close;
                    }
                });
            });
        });
}

fn render_empty_state(ui: &mut egui::Ui) {
    egui::Frame::NONE
        .fill(theme::PANEL_BG_ALT)
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(18))
        .show(ui, |ui| {
            ui.label(
                RichText::new("No saved sessions are available for this profile yet.")
                    .size(12.5)
                    .color(theme::FG_SOFT),
            );
        });
}

fn render_session_card(
    ui: &mut egui::Ui,
    session: &SessionSummary,
    selected: bool,
    current_session_id: Option<&str>,
) -> bool {
    let is_current = current_session_id == Some(session.session_id.as_str());
    let mut clicked = false;
    let frame_response = egui::Frame::NONE
        .fill(if selected {
            theme::blend(theme::PANEL_BG_ALT, theme::ACCENT, 0.16)
        } else {
            theme::PANEL_BG_ALT
        })
        .stroke(Stroke::new(
            1.0,
            if selected {
                theme::blend(theme::BORDER_STRONG, theme::ACCENT, 0.78)
            } else {
                theme::BORDER_SUBTLE
            },
        ))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                clicked = ui.radio(selected, "").clicked();
                ui.vertical(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(&session.label).size(14.5).strong().color(theme::FG));
                        if is_current {
                            ui.label(RichText::new("Current").size(10.5).color(theme::ACCENT).strong());
                        }
                        if session.is_live && !is_current {
                            ui.label(RichText::new("Live").size(10.5).color(theme::PALETTE_RED).strong());
                        }
                    });
                    ui.label(
                        RichText::new(format!(
                            "{} workspaces · {} panels · {}",
                            session.workspace_count,
                            session.panel_count,
                            format_relative_time(session.last_active_at)
                        ))
                        .size(12.0)
                        .color(theme::FG_SOFT),
                    );
                    ui.label(
                        RichText::new(format!(
                            "Session {}",
                            super::util::short_session_id(&session.session_id)
                        ))
                        .size(11.0)
                        .color(theme::FG_DIM)
                        .monospace(),
                    );
                });
            });
        })
        .response;

    let response = ui
        .interact(
            frame_response.rect,
            ui.make_persistent_id(("runtime_session_card", &session.session_id)),
            egui::Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);

    clicked || response.clicked()
}
