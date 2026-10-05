use egui::{Key, Modifiers, Response, Ui};
use horizon_core::{PanelId, Workspace, WorkspaceId};

use super::HorizonApp;
use crate::theme;

#[derive(Clone, Default)]
struct DestinationSearch {
    query: String,
    selected: Option<WorkspaceId>,
    last_frame: u64,
}

impl HorizonApp {
    pub(super) fn show_workspace_destination(
        &self,
        ui: &mut Ui,
        launcher: &Response,
        panel_id: PanelId,
        current: WorkspaceId,
    ) -> Option<WorkspaceId> {
        #[cfg(feature = "cloud-workspaces")]
        let can_move = !self.cloud_prototype.groups.contains_panel(&self.board, panel_id);
        #[cfg(not(feature = "cloud-workspaces"))]
        let can_move = true;
        let remote = self
            .board
            .panel(panel_id)
            .and_then(horizon_core::Panel::remote_workspace);
        render_destination_search(ui, launcher, &self.board.workspaces, current, |workspace| {
            let allowed = can_move
                && workspace
                    .remote_workspace
                    .as_ref()
                    .is_none_or(|reference| Some(reference) == remote);
            #[cfg(feature = "cloud-workspaces")]
            let allowed = allowed && !self.cloud_prototype.groups.contains_workspace(&workspace.local_id);
            allowed
        })
    }
}

fn render_destination_search(
    ui: &mut Ui,
    launcher: &Response,
    workspaces: &[Workspace],
    current: WorkspaceId,
    can_move: impl Fn(&Workspace) -> bool,
) -> Option<WorkspaceId> {
    let id = launcher.id.with(("workspace_destination", ui.ctx().viewport_id()));
    let frame = ui.ctx().cumulative_frame_nr();
    let previous = ui.data(|data| data.get_temp::<DestinationSearch>(id));
    let opening = launcher.secondary_clicked()
        || previous
            .as_ref()
            .is_none_or(|state| frame > state.last_frame.saturating_add(1));
    let mut state = if opening {
        DestinationSearch::default()
    } else {
        previous.unwrap_or_default()
    };
    ui.set_width((ui.ctx().content_rect().width() - 32.0).clamp(180.0, 300.0));
    ui.label(egui::RichText::new("Move to Workspace").size(12.0).color(theme::FG()));
    let query_id = id.with("query");
    let (down, up, enter) = if ui.memory(|memory| memory.has_focus(query_id)) {
        ui.input_mut(|input| {
            (
                input.consume_key(Modifiers::NONE, Key::ArrowDown),
                input.consume_key(Modifiers::NONE, Key::ArrowUp),
                input.consume_key(Modifiers::NONE, Key::Enter),
            )
        })
    } else {
        (false, false, false)
    };
    let search = ui.add(
        egui::TextEdit::singleline(&mut state.query)
            .id(query_id)
            .hint_text("Search workspaces…")
            .desired_width(f32::INFINITY),
    );
    if opening {
        search.request_focus();
    }
    let query = state.query.trim().to_lowercase();
    let matches: Vec<_> = workspaces
        .iter()
        .filter(|workspace| workspace.name.to_lowercase().contains(&query))
        .collect();
    let eligible: Vec<_> = matches
        .iter()
        .filter(|workspace| workspace.id != current && can_move(workspace))
        .map(|workspace| workspace.id)
        .collect();
    if search.changed() || !state.selected.is_some_and(|selected| eligible.contains(&selected)) {
        state.selected = eligible.first().copied();
    }
    if down {
        step_selection(&mut state.selected, &eligible, true);
    }
    if up {
        step_selection(&mut state.selected, &eligible, false);
    }
    let keyboard_moved = down || up;
    let mut chosen = if enter { state.selected } else { None };
    ui.label(
        egui::RichText::new(format!("{} of {} workspaces", matches.len(), workspaces.len()))
            .size(11.0)
            .color(theme::FG_SOFT()),
    );
    chosen = render_results(
        ui,
        &matches,
        &can_move,
        ResultPresentation {
            id,
            current,
            selected: state.selected,
            reset_scroll: opening || search.changed(),
            keyboard_moved,
        },
    )
    .or(chosen);
    if keyboard_moved {
        ui.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
    }
    ui.label(
        egui::RichText::new("↑ ↓ select   Enter move   Esc cancel")
            .size(11.0)
            .color(theme::FG_SOFT()),
    );
    state.last_frame = frame;
    ui.data_mut(|data| data.insert_temp(id, state));
    if chosen.is_some() {
        ui.close();
    }
    chosen
}

#[derive(Clone, Copy)]
struct ResultPresentation {
    id: egui::Id,
    current: WorkspaceId,
    selected: Option<WorkspaceId>,
    reset_scroll: bool,
    keyboard_moved: bool,
}

fn render_results(
    ui: &mut Ui,
    matches: &[&Workspace],
    can_move: &impl Fn(&Workspace) -> bool,
    presentation: ResultPresentation,
) -> Option<WorkspaceId> {
    let ResultPresentation {
        id,
        current,
        selected,
        reset_scroll,
        keyboard_moved,
    } = presentation;
    let height = (ui.ctx().content_rect().height() - 280.0).clamp(48.0, 240.0);
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(id.with("results"))
        .max_height(height)
        .auto_shrink([false, true]);
    if reset_scroll {
        scroll = scroll.vertical_scroll_offset(0.0);
    }
    let mut chosen = None;
    scroll.show(ui, |ui| {
        if matches.is_empty() {
            ui.label("No matching workspaces. Try another name.");
        }
        for workspace in matches {
            let is_current = workspace.id == current;
            let enabled = !is_current && can_move(workspace);
            let selected = enabled && selected == Some(workspace.id);
            let name = if is_current {
                format!("{} (current)", workspace.name)
            } else {
                workspace.name.clone()
            };
            let mut text = egui::text::LayoutJob::default();
            text.append(
                "●  ",
                0.0,
                egui::text::TextFormat {
                    font_id: egui::FontId::proportional(12.0),
                    color: theme::workspace_accent(workspace.color_idx),
                    ..Default::default()
                },
            );
            text.append(
                &name,
                0.0,
                egui::text::TextFormat {
                    font_id: egui::FontId::proportional(12.0),
                    color: if selected { theme::FG() } else { theme::FG_SOFT() },
                    ..Default::default()
                },
            );
            let response = ui
                .add_enabled(
                    enabled,
                    egui::Button::new(text)
                        .selected(selected)
                        .frame(selected)
                        .truncate()
                        .min_size(egui::vec2(ui.available_width(), 28.0)),
                )
                .on_hover_text(&name)
                .on_disabled_hover_text(if is_current {
                    "This panel is already in this workspace."
                } else {
                    "This panel cannot move into this environment."
                });
            if keyboard_moved && selected {
                response.scroll_to_me(Some(egui::Align::Center));
            }
            if response.clicked() {
                chosen = Some(workspace.id);
            }
        }
    });
    chosen
}

fn step_selection(selected: &mut Option<WorkspaceId>, eligible: &[WorkspaceId], forward: bool) {
    if eligible.is_empty() {
        *selected = None;
        return;
    }
    let index = selected
        .and_then(|id| eligible.iter().position(|candidate| *candidate == id))
        .unwrap_or(0);
    let next = if forward {
        (index + 1) % eligible.len()
    } else {
        (index + eligible.len() - 1) % eligible.len()
    };
    *selected = Some(eligible[next]);
}

#[cfg(test)]
mod tests;
