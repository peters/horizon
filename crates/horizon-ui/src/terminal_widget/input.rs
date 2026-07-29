use alacritty_terminal::term::TermMode;
use egui::emath::TSTransform;
use egui::{Key, PointerButton, Pos2, Vec2};
use horizon_core::Panel;

mod frame_events;
mod keyboard;
mod routing;
mod selection_drag;
mod viewport_scroll;

use super::super::input;
use super::super::primary_selection::PrimarySelection;

use self::frame_events::{PointerFrameEvents, final_primary_release_index, pointer_event_targets_rect, transform_pos};
pub(crate) use self::keyboard::SSH_RECONNECT_SHORTCUT;
pub(super) use self::keyboard::handle_terminal_keyboard_input;
pub(super) use self::routing::pty_mouse_reporting_enabled;
use self::routing::{
    local_primary_selection_allowed, pointer_button_event_needs_handling, pointer_button_routes_to_pty_mouse,
    pointer_button_starts_local_selection, pointer_motion_routes_to_pty_mouse,
};
use self::selection_drag::SelectionFrameOutcome;
pub(crate) use self::selection_drag::TerminalSelectionDragState;
use self::viewport_scroll::{
    SELECTION_AUTO_SCROLL_INTERVAL, handle_pointer_selection_drag, start_local_selection_at,
    update_pointer_selection_endpoint,
};
use super::layout::{GridMetrics, TerminalInteraction, grid_point_from_position};
use super::scrollbar::{scrollbar_pointer_to_scrollback, scrollbar_thumb_height};

pub(super) struct PointerSupport<'a> {
    pub metrics: &'a GridMetrics,
    pub visible_rows: u16,
    pub visible_cols: u16,
    pub primary_selection: &'a PrimarySelection,
    pub selection_drag: &'a mut TerminalSelectionDragState,
}

struct PointerContext<'a> {
    interaction: &'a TerminalInteraction,
    metrics: &'a GridMetrics,
    visible_rows: u16,
    visible_cols: u16,
    terminal_mode: TermMode,
    pointer_buttons: input::PointerButtons,
    current_modifiers: egui::Modifiers,
    hovered_point: Option<input::GridPoint>,
    from_global: Option<TSTransform>,
    active_pointer_pos: Option<Pos2>,
    primary_selection: &'a PrimarySelection,
    ui_ctx: egui::Context,
}

pub(super) fn handle_terminal_pointer_input(
    ui: &mut egui::Ui,
    panel: &mut Panel,
    interaction: &TerminalInteraction,
    is_active_panel: bool,
    support: PointerSupport<'_>,
) {
    let panel_id = panel.id;
    let PointerSupport {
        metrics,
        visible_rows,
        visible_cols,
        primary_selection,
        selection_drag,
    } = support;
    if interaction.body.clicked() {
        interaction.body.request_focus();
    }
    if is_active_panel && ui.input(|input| input.key_pressed(Key::Tab)) {
        interaction.body.request_focus();
    }

    let from_global = ui.ctx().layer_transform_from_global(ui.layer_id());

    if !should_handle_terminal_pointer(ui, interaction, from_global, selection_drag.active_for(panel_id)) {
        return;
    }

    // Only clone events for the panel that actually needs pointer processing.
    let events: Vec<egui::Event> = ui.input(|input| input.events.clone());
    let frame_events = PointerFrameEvents::collect(&events, from_global, interaction.layout.body);

    let Some(terminal_mode) = panel.terminal_mut().map(|terminal| terminal.mode()) else {
        return;
    };
    let pointer_buttons = ui.input(|input| input::PointerButtons {
        primary: input.pointer.primary_down(),
        middle: input.pointer.middle_down(),
        secondary: input.pointer.secondary_down(),
    });
    let current_modifiers = ui.input(|input| input.modifiers);
    let active_pointer_pos = ui
        .input(|input| input.pointer.interact_pos())
        .map(|position| transform_pos(from_global, position));
    let hovered_point = interaction
        .body
        .hover_pos()
        .filter(|position| interaction.layout.body.contains(*position))
        .and_then(|position| {
            grid_point_from_position(interaction.layout.body, position, metrics, visible_rows, visible_cols)
        });
    let pointer_context = PointerContext {
        interaction,
        metrics,
        visible_rows,
        visible_cols,
        terminal_mode,
        pointer_buttons,
        current_modifiers,
        hovered_point,
        from_global,
        active_pointer_pos,
        primary_selection,
        ui_ctx: ui.ctx().clone(),
    };
    let selection_drag_threshold = ui.ctx().options(|options| options.input_options.max_click_dist);

    let selection_outcome = handle_terminal_body_pointer_actions(
        panel,
        &pointer_context,
        &frame_events,
        selection_drag,
        selection_drag_threshold,
    );
    let release_replay_index = if selection_outcome.release_completes_drag {
        final_primary_release_index(&events)
    } else {
        None
    };
    let local_scrollback_changed = handle_pointer_events(
        &events,
        panel,
        &pointer_context,
        selection_outcome.claimed_primary_pointer,
        release_replay_index,
    );
    update_active_selection_after_scrollback(local_scrollback_changed, panel, &pointer_context, selection_drag);
    maybe_copy_selection_to_primary(
        panel,
        interaction,
        primary_selection,
        selection_outcome.copy_completed_selection,
    );

    handle_scrollbar_drag(ui, panel, interaction, visible_rows);

    // Show pointing hand when Ctrl/Cmd hovering over clickable content.
    if ui.input(|input| input.modifiers.ctrl || input.modifiers.command)
        && let Some(point) = pointer_context.hovered_point
        && let Some(terminal) = panel.terminal()
        && terminal.clickable_at_point(point.line, point.column).is_some()
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}

fn update_active_selection_after_scrollback(
    scrollback_changed: bool,
    panel: &mut Panel,
    pointer: &PointerContext<'_>,
    selection_drag: &TerminalSelectionDragState,
) {
    if scrollback_changed
        && selection_drag.active_for(panel.id)
        && pointer.pointer_buttons.primary
        && panel.terminal().is_some_and(horizon_core::Terminal::has_selection)
        && let Some(pos) = pointer
            .active_pointer_pos
            .or_else(|| response_pointer_pos(&pointer.interaction.body))
    {
        update_pointer_selection_endpoint(
            panel,
            pos,
            pointer.interaction.layout.body,
            pointer.metrics,
            pointer.visible_rows,
            pointer.visible_cols,
        );
    }
}

fn should_handle_terminal_pointer(
    ui: &egui::Ui,
    interaction: &TerminalInteraction,
    from_global: Option<TSTransform>,
    selection_drag_active: bool,
) -> bool {
    // Check cheap interaction-state conditions before cloning the event list.
    // For most panels the pointer is elsewhere, so we exit early and avoid the
    // per-panel Vec<Event> clone entirely.
    response_pointer_pos(&interaction.body).is_some()
        || response_pointer_pos(&interaction.scrollbar).is_some()
        || interaction.body.is_pointer_button_down_on()
        || interaction.scrollbar.is_pointer_button_down_on()
        || interaction.body.drag_stopped_by(PointerButton::Primary)
        || interaction.body.double_clicked()
        || interaction.body.triple_clicked()
        || interaction.body.clicked_by(PointerButton::Middle)
        || interaction.scrollbar.clicked()
        || selection_drag_active
        || ui.input(|input| {
            pointer_event_targets_rect(&input.events, from_global, interaction.layout.body)
                || pointer_event_targets_rect(&input.events, from_global, interaction.layout.scrollbar)
        })
}

fn handle_pointer_events(
    events: &[egui::Event],
    panel: &mut Panel,
    pointer: &PointerContext<'_>,
    local_primary_selection_claimed: bool,
    release_replay_index: Option<usize>,
) -> bool {
    let mut local_scrollback_changed = false;
    for (index, event) in events.iter().enumerate() {
        match event {
            egui::Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers,
            } => {
                if local_primary_selection_claimed && *button == PointerButton::Primary {
                    // Replay selection endpoints for presses and drag-completing
                    // releases that follow a same-frame scrollback change, so
                    // they anchor in the viewport the user actually saw.
                    let pos = transform_pos(pointer.from_global, *pos);
                    if *pressed {
                        if local_scrollback_changed && local_primary_selection_allowed(*modifiers) {
                            start_local_selection_at(panel, pointer, pos);
                        }
                    } else if local_scrollback_changed && release_replay_index == Some(index) {
                        update_pointer_selection_endpoint(
                            panel,
                            pos,
                            pointer.interaction.layout.body,
                            pointer.metrics,
                            pointer.visible_rows,
                            pointer.visible_cols,
                        );
                        local_scrollback_changed = false;
                    }
                    continue;
                }
                if !pointer_button_event_needs_handling(pointer.terminal_mode, *button, *pressed, *modifiers) {
                    continue;
                }
                let pos = transform_pos(pointer.from_global, *pos);
                if !pointer.interaction.layout.body.contains(pos) {
                    continue;
                }
                if *pressed {
                    pointer.interaction.body.request_focus();
                }
                handle_pointer_button(panel, pointer, pos, *button, *pressed, *modifiers);
            }
            egui::Event::PointerMoved(pos) => {
                if local_primary_selection_claimed && pointer.pointer_buttons.primary {
                    continue;
                }
                let pos = transform_pos(pointer.from_global, *pos);
                let inside = pointer.interaction.layout.body.contains(pos);
                if inside
                    && pointer_motion_routes_to_pty_mouse(
                        pointer.terminal_mode,
                        pointer.pointer_buttons,
                        pointer.current_modifiers,
                    )
                    && let Some(point) = grid_point_from_position(
                        pointer.interaction.layout.body,
                        pos,
                        pointer.metrics,
                        pointer.visible_rows,
                        pointer.visible_cols,
                    )
                    && let Some(bytes) = input::mouse_motion_report(
                        pointer.pointer_buttons,
                        pointer.current_modifiers,
                        pointer.terminal_mode,
                        point,
                    )
                    && !bytes.is_empty()
                {
                    panel.write_input(&bytes);
                }
            }
            egui::Event::MouseWheel { delta, unit, modifiers } => {
                if modifiers.ctrl || modifiers.command {
                    continue;
                }
                if let Some(point) = pointer.hovered_point
                    && let Some(action) = input::wheel_action(
                        *delta,
                        *unit,
                        Vec2::new(pointer.metrics.char_width, pointer.metrics.line_height),
                        *modifiers,
                        pointer.terminal_mode,
                        point,
                    )
                {
                    match action {
                        input::WheelAction::Pty(bytes) if !bytes.is_empty() => panel.write_input(&bytes),
                        input::WheelAction::Pty(_) => {}
                        input::WheelAction::Scrollback(lines) => {
                            let scrollback_before = panel.terminal().map_or(0, horizon_core::Terminal::scrollback);
                            panel.scroll_scrollback_by(lines);
                            local_scrollback_changed |=
                                panel.terminal().map_or(0, horizon_core::Terminal::scrollback) != scrollback_before;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    local_scrollback_changed
}

fn handle_terminal_body_pointer_actions(
    panel: &mut Panel,
    pointer: &PointerContext<'_>,
    frame_events: &PointerFrameEvents,
    selection_drag: &mut TerminalSelectionDragState,
    selection_drag_threshold: f32,
) -> SelectionFrameOutcome {
    let mut outcome = SelectionFrameOutcome {
        claimed_primary_pointer: selection_drag.active_for(panel.id),
        ..SelectionFrameOutcome::default()
    };
    let body_pointer_pos = pointer
        .active_pointer_pos
        .or_else(|| response_pointer_pos(&pointer.interaction.body));

    if frame_events.body_middle_press_pos.is_some()
        && !pty_mouse_reporting_enabled(pointer.terminal_mode, pointer.current_modifiers)
        && should_request_primary_paste(PointerButton::Middle, true, pointer.current_modifiers)
    {
        pointer
            .primary_selection
            .request_paste(panel.id, pointer.ui_ctx.clone());
        return outcome;
    }

    let mut drag_restarted_by_press = false;
    if let Some(pos) = frame_events.body_primary_press_pos
        && pointer_button_starts_local_selection(
            pointer.terminal_mode,
            PointerButton::Primary,
            true,
            pointer.current_modifiers,
        )
    {
        pointer.interaction.body.request_focus();
        handle_pointer_button(
            panel,
            pointer,
            pos,
            PointerButton::Primary,
            true,
            pointer.current_modifiers,
        );
        selection_drag.start(panel.id, pos);
        outcome.claimed_primary_pointer = true;
        drag_restarted_by_press = true;
    }

    // A release only completes the drag when no later in-body press restarted
    // it; a same-frame release followed by a fresh press hands the still-held
    // pointer to the new selection instead of finishing it.
    outcome.release_completes_drag = frame_events.primary_release_ends_frame
        || (frame_events.primary_release_pos.is_some() && !drag_restarted_by_press);

    if selection_drag.active_for(panel.id)
        && pointer.pointer_buttons.primary
        && panel.terminal().is_some_and(horizon_core::Terminal::has_selection)
        && let Some(pos) = body_pointer_pos
    {
        selection_drag.mark_dragged(panel.id, pos, selection_drag_threshold);
        let auto_scrolled = handle_pointer_selection_drag(
            panel,
            pos,
            pointer.interaction.layout.body,
            pointer.metrics,
            pointer.visible_rows,
            pointer.visible_cols,
        );
        if auto_scrolled {
            pointer.ui_ctx.request_repaint_after(SELECTION_AUTO_SCROLL_INTERVAL);
        }
    }

    if selection_drag.active_for(panel.id)
        && outcome.release_completes_drag
        && panel.terminal().is_some_and(horizon_core::Terminal::has_selection)
        && let Some(pos) = frame_events.primary_release_pos.or(body_pointer_pos)
    {
        selection_drag.mark_dragged(panel.id, pos, selection_drag_threshold);
        handle_pointer_selection_drag(
            panel,
            pos,
            pointer.interaction.layout.body,
            pointer.metrics,
            pointer.visible_rows,
            pointer.visible_cols,
        );
    }

    if selection_drag.active_for(panel.id) && (outcome.release_completes_drag || !pointer.pointer_buttons.primary) {
        outcome.copy_completed_selection = selection_drag.finish(panel.id);
    }

    outcome
}

fn handle_pointer_button(
    panel: &mut Panel,
    pointer: &PointerContext<'_>,
    pos: Pos2,
    button: egui::PointerButton,
    pressed: bool,
    modifiers: egui::Modifiers,
) {
    // Ctrl+click / Cmd+click opens URLs and file paths regardless of mouse mode.
    if (modifiers.ctrl || modifiers.command)
        && button == egui::PointerButton::Primary
        && pressed
        && let Some(point) = grid_point_from_position(
            pointer.interaction.layout.body,
            pos,
            pointer.metrics,
            pointer.visible_rows,
            pointer.visible_cols,
        )
        && let Some(terminal) = panel.terminal()
        && let Some(target) = terminal.clickable_at_point(point.line, point.column)
    {
        horizon_core::open_url(&target);
        return;
    }

    if pointer_button_starts_local_selection(pointer.terminal_mode, button, pressed, modifiers) {
        start_local_selection_at(panel, pointer, pos);
    } else if pointer_button_routes_to_pty_mouse(pointer.terminal_mode, button, modifiers)
        && let Some(point) = grid_point_from_position(
            pointer.interaction.layout.body,
            pos,
            pointer.metrics,
            pointer.visible_rows,
            pointer.visible_cols,
        )
        && let Some(bytes) = input::mouse_button_report(button, pressed, modifiers, pointer.terminal_mode, point)
        && !bytes.is_empty()
    {
        panel.write_input(&bytes);
    }
}

fn maybe_copy_selection_to_primary(
    panel: &Panel,
    interaction: &TerminalInteraction,
    primary_selection: &PrimarySelection,
    selection_drag_completed: bool,
) {
    if !selection_copy_completed(
        selection_drag_completed || interaction.body.drag_stopped_by(PointerButton::Primary),
        interaction.body.double_clicked(),
        interaction.body.triple_clicked(),
    ) {
        return;
    }

    if let Some(text) = panel.terminal().and_then(horizon_core::Terminal::selection_to_string) {
        primary_selection.copy(&text);
    }
}

fn selection_copy_completed(drag_stopped: bool, double_clicked: bool, triple_clicked: bool) -> bool {
    drag_stopped || double_clicked || triple_clicked
}

fn should_request_primary_paste(button: egui::PointerButton, pressed: bool, modifiers: egui::Modifiers) -> bool {
    cfg!(target_os = "linux")
        && button == egui::PointerButton::Middle
        && pressed
        && !modifiers.ctrl
        && !modifiers.command
}

fn handle_scrollbar_drag(ui: &mut egui::Ui, panel: &mut Panel, interaction: &TerminalInteraction, visible_rows: u16) {
    let from_global = ui.ctx().layer_transform_from_global(ui.layer_id());
    if (interaction.scrollbar.dragged() || interaction.scrollbar.clicked())
        && let Some(pointer_position) = ui
            .input(|input| input.pointer.interact_pos())
            .map(|position| transform_pos(from_global, position))
    {
        let history_size = panel.terminal().map_or(0, horizon_core::Terminal::history_size);
        let target_scrollback = scrollbar_pointer_to_scrollback(
            pointer_position,
            interaction.scrollbar.rect.shrink2(Vec2::new(2.0, 2.0)),
            scrollbar_thumb_height(interaction.scrollbar.rect.height() - 4.0, visible_rows, history_size),
            history_size,
        );
        panel.set_scrollback(target_scrollback);
    }
}

fn response_pointer_pos(response: &egui::Response) -> Option<Pos2> {
    response.interact_pointer_pos().or_else(|| response.hover_pos())
}

#[cfg(test)]
mod mouse_tests;

#[cfg(test)]
mod selection_tests;

#[cfg(test)]
mod tests {
    use super::{selection_copy_completed, should_request_primary_paste};
    use egui::{Modifiers, PointerButton};

    #[test]
    fn middle_click_requests_primary_paste_only_on_linux_without_ctrl_or_cmd() {
        assert_eq!(
            should_request_primary_paste(PointerButton::Middle, true, Modifiers::NONE),
            cfg!(target_os = "linux")
        );
    }

    #[test]
    fn middle_click_does_not_request_primary_paste_with_ctrl_or_cmd() {
        assert!(!should_request_primary_paste(
            PointerButton::Middle,
            true,
            Modifiers::CTRL
        ));
        assert!(!should_request_primary_paste(
            PointerButton::Middle,
            true,
            Modifiers::COMMAND
        ));
    }

    #[test]
    fn selection_completion_triggers_primary_copy() {
        assert!(selection_copy_completed(true, false, false));
        assert!(selection_copy_completed(false, true, false));
        assert!(selection_copy_completed(false, false, true));
        assert!(!selection_copy_completed(false, false, false));
    }
}
