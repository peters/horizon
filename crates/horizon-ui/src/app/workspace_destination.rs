use egui::{
    Align, Color32, CornerRadius, Key, Layout, Modifiers, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui,
    UiBuilder, Vec2,
};
use horizon_core::{PanelId, Workspace, WorkspaceId};

use super::HorizonApp;
use crate::theme;

const MENU_SEARCH_HEIGHT: f32 = 32.0;
const MENU_SEARCH_RADIUS: u8 = 8;
const MENU_SEARCH_TEXT_INSET: f32 = 28.0;

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
    let search = render_menu_search_field(ui, &mut state.query, query_id, opening);
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

fn render_menu_search_field(ui: &mut Ui, query: &mut String, id: egui::Id, opening: bool) -> Response {
    let width = ui.available_width();
    // `Sense::CLICK` is not focusable. The text edit is the field's only focus target.
    let (rect, well) = ui.allocate_exact_size(Vec2::new(width, MENU_SEARCH_HEIGHT), Sense::CLICK);
    let _ = well.clone().on_hover_cursor(egui::CursorIcon::Text);
    let focused = opening || ui.memory(|memory| memory.has_focus(id));
    let hovered = ui
        .input(|input| input.pointer.hover_pos())
        .is_some_and(|pos| rect.contains(pos));
    paint_menu_search_well(ui.painter(), rect, focused, hovered);
    paint_search_mark(
        ui.painter(),
        Pos2::new(rect.min.x + 15.0, rect.center().y),
        if focused || !query.is_empty() {
            theme::FG()
        } else {
            theme::FG_SOFT()
        },
    );

    let text_rect = Rect::from_min_max(
        Pos2::new(rect.min.x + MENU_SEARCH_TEXT_INSET, rect.min.y + 2.0),
        Pos2::new(rect.max.x - 10.0, rect.max.y - 2.0),
    );
    let mut editor = ui.new_child(
        UiBuilder::new()
            .max_rect(text_rect)
            .layout(Layout::left_to_right(Align::Center))
            .id_salt(id.with("editor")),
    );
    // egui replaces any hint color with `weak_text_color`. Set it here so the
    // placeholder stays `FG_DIM` and does not inherit the menu's text color.
    editor.visuals_mut().weak_text_color = Some(theme::FG_DIM());
    let response = editor.add(
        egui::TextEdit::singleline(query)
            .id(id)
            .frame(egui::Frame::NONE)
            .desired_width(text_rect.width())
            .min_size(text_rect.size())
            .font(egui::FontId::proportional(13.0))
            .text_color(theme::FG())
            .vertical_align(Align::Center)
            .margin(egui::Margin::ZERO)
            .hint_text(egui::RichText::new("Search workspaces…").size(12.0)),
    );
    if well.clicked() {
        response.request_focus();
    }
    response
}

fn paint_menu_search_well(painter: &egui::Painter, rect: Rect, focused: bool, hovered: bool) {
    let stroke = if focused {
        theme::alpha(theme::ACCENT(), 200)
    } else if hovered {
        theme::alpha(theme::ACCENT(), 160)
    } else {
        theme::alpha(theme::BORDER_STRONG(), 200)
    };
    painter.rect(
        rect,
        CornerRadius::same(MENU_SEARCH_RADIUS),
        theme::BG(),
        Stroke::new(1.0, stroke),
        StrokeKind::Inside,
    );
    painter.line_segment(
        [
            Pos2::new(rect.min.x + 12.0, rect.min.y + 2.0),
            Pos2::new(rect.max.x - 12.0, rect.min.y + 2.0),
        ],
        Stroke::new(1.0, theme::alpha(theme::FG(), if focused { 28 } else { 16 })),
    );
}

fn paint_search_mark(painter: &egui::Painter, center: Pos2, color: Color32) {
    let loop_center = Pos2::new(center.x - 1.2, center.y - 1.2);
    painter.circle_stroke(loop_center, 3.6, Stroke::new(1.25, color));
    painter.line_segment(
        [
            Pos2::new(loop_center.x + 2.7, loop_center.y + 2.7),
            Pos2::new(loop_center.x + 5.6, loop_center.y + 5.6),
        ],
        Stroke::new(1.25, color),
    );
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
