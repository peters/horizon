//! The row of a panel under its work item: its close control, its press and its
//! hover text.
//!
//! The row draws its look from `panel_rows` and answers with a focus, a pan or a
//! close in the actions of the sidebar.

use egui::{CursorIcon, Pos2, Rect, Sense, Vec2};

use super::super::HorizonApp;
use super::{SidebarActions, SidebarPanelEntry, WorkspaceSidebarEntry, panel_rows, rows, sidebar_panel_row_id, text};

impl HorizonApp {
    pub(super) fn render_sidebar_panel(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &WorkspaceSidebarEntry,
        panel: &SidebarPanelEntry,
        last: bool,
        actions: &mut SidebarActions,
    ) {
        let row_rect = ui
            .allocate_space(Vec2::new(ui.available_width(), panel_rows::PANEL_ROW_HEIGHT))
            .1;
        let label = &panel.label;
        let hovered = ui.rect_contains_pointer(row_rect);
        let close_rect = panel_rows::close_rect(row_rect);
        // The close button always takes clicks and tells a screen reader its name,
        // but shows only on the hovered or focused row, or while the row or the
        // button has keyboard focus.
        let close = ui
            .interact(
                close_rect,
                ui.make_persistent_id(("sidebar_panel_close", panel.id.0)),
                Sense::click(),
            )
            .on_hover_text("Close panel");
        close.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Close panel"));
        let row_id = sidebar_panel_row_id(panel.id);
        let row_focused = ui.memory(|memory| memory.has_focus(row_id));
        let close_shows = hovered || panel.is_focused || row_focused || close.hovered() || close.has_focus();
        // A pointer press closes only a button that the person could see before the
        // press began: a tap on the hidden button of another row focuses that row
        // instead. The decision is kept from pointer-down until the release.
        let shown_id = ui.make_persistent_id(("sidebar_panel_close_shown", panel.id.0));
        let armed_id = ui.make_persistent_id(("sidebar_panel_close_armed", panel.id.0));
        let shown_before = ui.ctx().data(|data| data.get_temp::<bool>(shown_id)).unwrap_or(false);
        // A press and its release can land in one frame: a quick click of a person is
        // inside one frame of 55 ms, and the pointer clears its press origin on the
        // release. The press of this frame is read from its events as well, so a
        // press on the mark arms it at any frame rate.
        let (pressed, press_origin) = ui.input(|input| {
            let origin = input.pointer.press_origin().or_else(|| {
                input.events.iter().find_map(|event| match event {
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        ..
                    } => Some(*pos),
                    _ => None,
                })
            });
            (input.pointer.any_pressed(), origin)
        });
        if pressed && press_origin.is_some_and(|origin| close_rect.contains(origin)) {
            ui.ctx().data_mut(|data| data.insert_temp(armed_id, shown_before));
        }
        ui.ctx().data_mut(|data| data.insert_temp(shown_id, close_shows));
        let armed = ui.ctx().data(|data| data.get_temp::<bool>(armed_id)).unwrap_or(false);
        let by_pointer = ui.input(|input| input.pointer.any_click());
        let close_clicked = close.clicked() && (armed || close.has_focus() || !by_pointer);
        let blind_tap = close.clicked() && !close_clicked;
        let look = panel_rows::PanelRowLook {
            guide_x: panel_rows::guide_x(rows::card(row_rect)),
            accent: workspace.color,
            kind: panel.kind,
            focused: panel.is_focused,
            label,
            attention: panel.attention,
        };
        let title_elided = panel_rows::paint_panel_row(
            ui,
            row_rect,
            &look,
            panel_rows::PanelRowState {
                hovered,
                close: close_shows.then(|| close.hovered() || close.has_focus()),
                last,
            },
        );

        let row_click_rect = Rect::from_min_max(row_rect.min, Pos2::new(close_rect.min.x, row_rect.max.y));
        let row_response = ui.interact(row_click_rect, row_id, Sense::click());
        row_response.widget_info(|| {
            let spoken = text::panel_spoken(panel.kind, label, panel.attention);
            egui::WidgetInfo::labeled(egui::Role::Button, true, spoken)
        });
        // The whole title, when the row cuts it or shows only the agent's task,
        // and the sentence of the state while the pointer is on the row.
        let state = (hovered && panel.attention.word().is_some()).then(|| panel.attention.sentence());
        let row_response = match (title_elided || *label != panel.title, state) {
            (true, Some(sentence)) => row_response.on_hover_text(format!("{}\n{sentence}", panel.title)),
            (true, None) => row_response.on_hover_text(&panel.title),
            (false, Some(sentence)) => row_response.on_hover_text(sentence),
            (false, None) => row_response,
        };
        if hovered {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }

        if close_clicked {
            actions.close_panel = Some(panel.id);
        } else if row_response.clicked() || blind_tap {
            // A click on the row is a look at the panel, so its state clears.
            self.note_panel_attention_look(panel.id);
            actions.focus_panel = Some(panel.id);
            actions.pan_to_panel = Some(panel.id);
        }

        self.show_sidebar_panel_context_menu(&row_response, workspace, panel.id, panel.kind, actions);
        ui.add_space(1.0);
    }
}
