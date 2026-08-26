use alacritty_terminal::term::TermMode;
use egui::emath::TSTransform;
use egui::{Key, PointerButton, Pos2, Vec2};
use horizon_core::{Panel, PanelId};

mod frame_events;
mod keyboard;
mod routing;
mod selection_drag;
mod viewport_scroll;

use super::super::input;
use super::super::primary_selection::PrimarySelection;

use self::frame_events::{LocalSelectionEventTracker, PointerFrameEvents, pointer_event_targets_rect, transform_pos};
pub(crate) use self::keyboard::SSH_RECONNECT_SHORTCUT;
pub(super) use self::keyboard::handle_terminal_keyboard_input;
pub(super) use self::routing::pty_mouse_reporting_enabled;
use self::routing::{
    pointer_button_event_needs_handling, pointer_button_opens_osc8_hyperlink, pointer_button_routes_to_pty_mouse,
    pointer_button_starts_local_selection, pointer_motion_routes_to_pty_mouse,
};
pub(crate) use self::selection_drag::TerminalSelectionDragState;
use self::selection_drag::{AutoScrollCadence, CapturedPrimaryGesture, SelectionFrameOutcome};
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
    frame_time: f64,
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

    selection_drag.expire_completed_primary_gesture(ui.ctx().cumulative_frame_nr());
    let from_global = ui.ctx().layer_transform_from_global(ui.layer_id());

    let pointer_gesture_active =
        selection_drag.active_for(panel_id) || selection_drag.has_primary_gesture_for(panel_id);
    if !should_handle_terminal_pointer(ui, interaction, from_global, pointer_gesture_active) {
        return;
    }

    // Only clone events for the panel that actually needs pointer processing.
    let events: Vec<egui::Event> = ui.input(|input| input.events.clone());
    selection_drag.cancel_interrupted_primary_gesture(
        ui.input(|input| input.pointer.primary_down()),
        has_primary_release(&events),
        events.iter().any(|event| matches!(event, egui::Event::PointerMoved(_))),
    );
    let Some(terminal_mode) = panel.terminal_mut().map(|terminal| terminal.mode()) else {
        return;
    };
    let frame_events = PointerFrameEvents::collect(&events, from_global, interaction.layout.body, terminal_mode);
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
        frame_time: ui.input(|input| input.time),
    };
    let selection_drag_threshold = ui.ctx().options(|options| options.input_options.max_click_dist);

    let selection_active_at_frame_start = selection_drag.active_for(panel_id);
    let selection_outcome = handle_terminal_body_pointer_actions(
        panel,
        &pointer_context,
        &frame_events,
        selection_drag,
        selection_drag_threshold,
    );
    let local_scrollback_changed = handle_pointer_events(
        &events,
        panel,
        &pointer_context,
        selection_drag,
        selection_active_at_frame_start,
        &selection_outcome,
    );
    update_active_selection_after_scrollback(local_scrollback_changed, panel, &pointer_context, selection_drag);
    maybe_copy_selection_to_primary(
        panel,
        interaction,
        primary_selection,
        selection_outcome.copy_completed_selection,
    );

    handle_scrollbar_drag(ui, panel, interaction, visible_rows);

    // OSC 8 hyperlinks are plain-clickable; URLs and paths still need Ctrl/Cmd.
    if let Some(point) = pointer_context.hovered_point
        && let Some(terminal) = panel.terminal()
    {
        let modifiers = ui.input(|input| input.modifiers);
        let has_hyperlink = terminal.has_hyperlink_at_point(point.line, point.column);
        if (pointer_button_opens_osc8_hyperlink(PointerButton::Primary, modifiers) && has_hyperlink)
            || ((modifiers.ctrl || modifiers.command)
                && (has_hyperlink || terminal.clickable_at_point(point.line, point.column).is_some()))
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
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
    pointer_gesture_active: bool,
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
        || pointer_gesture_active
        || ui.input(|input| {
            pointer_event_targets_rect(&input.events, from_global, interaction.layout.body)
                || pointer_event_targets_rect(&input.events, from_global, interaction.layout.scrollbar)
        })
}

fn handle_pointer_events(
    events: &[egui::Event],
    panel: &mut Panel,
    pointer: &PointerContext<'_>,
    selection_drag: &mut TerminalSelectionDragState,
    selection_active_at_frame_start: bool,
    selection_outcome: &SelectionFrameOutcome,
) -> bool {
    let mut local_scrollback_changed = false;
    let mut local_selection_events = LocalSelectionEventTracker::new(selection_active_at_frame_start);
    let mut event_pointer_buttons = pointer_buttons_at_frame_start(events, pointer.pointer_buttons);
    let mut event_modifiers = pointer.current_modifiers;
    let body = pointer.interaction.layout.body;
    let frame = pointer.ui_ctx.cumulative_frame_nr();
    for (index, event) in events.iter().enumerate() {
        if let Some(modifiers) = modifiers_for_event(event) {
            event_modifiers = modifiers;
        }
        let pointer_buttons_before_event = event_pointer_buttons;
        update_pointer_buttons_for_event(&mut event_pointer_buttons, event);
        let local_selection_claimed =
            local_selection_events.claims(event, pointer.from_global, body, pointer.terminal_mode);
        match event {
            egui::Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers,
            } => {
                let pos = transform_pos(pointer.from_global, *pos);
                let primary_released = *button == PointerButton::Primary && !*pressed;
                if primary_released
                    && selection_drag
                        .primary_gesture_for_event(panel.id, frame, index)
                        .is_some()
                {
                    finish_captured_primary_gesture(panel, pointer, selection_drag, pos, index);
                    continue;
                }
                if local_selection_claimed {
                    if selection_outcome.osc8_replay_index == Some(index)
                        && replay_osc8_press(panel, pointer, selection_drag, pos, *modifiers, index)
                    {
                        continue;
                    }
                    if *pressed && local_scrollback_changed {
                        start_local_selection_at(panel, pointer, pos);
                    } else if local_scrollback_changed && selection_outcome.release_replay_index == Some(index) {
                        update_pointer_selection_endpoint(
                            panel,
                            pos,
                            body,
                            pointer.metrics,
                            pointer.visible_rows,
                            pointer.visible_cols,
                        );
                        local_scrollback_changed = false;
                    }
                    continue;
                }
                if primary_released {
                    continue;
                }
                if !body.contains(pos)
                    || (*pressed && !body_layer_is_topmost_at(pointer, pos))
                    || !pointer_button_event_needs_handling(pointer.terminal_mode, *button, *pressed, *modifiers)
                {
                    continue;
                }
                if let Some(route) = handle_pointer_button(panel, pointer, pos, *button, *pressed, *modifiers) {
                    selection_drag.capture_primary_gesture(panel.id, route, frame, index);
                }
            }
            egui::Event::PointerMoved(pos) if !local_selection_claimed => {
                handle_pointer_motion(
                    panel,
                    pointer,
                    selection_drag,
                    *pos,
                    pointer_buttons_before_event,
                    event_modifiers,
                    index,
                );
            }
            egui::Event::MouseWheel {
                delta, unit, modifiers, ..
            } => {
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
                    local_scrollback_changed |= apply_wheel_action(panel, action);
                }
            }
            _ => {}
        }
    }
    local_scrollback_changed
}

fn apply_wheel_action(panel: &mut Panel, action: input::WheelAction) -> bool {
    match action {
        input::WheelAction::Pty(bytes) => {
            if !bytes.is_empty() {
                panel.write_input(&bytes);
            }
            false
        }
        input::WheelAction::Scrollback(lines) => {
            let before = panel.terminal().map_or(0, horizon_core::Terminal::scrollback);
            panel.scroll_scrollback_by(lines);
            panel.terminal().map_or(0, horizon_core::Terminal::scrollback) != before
        }
    }
}

fn finish_captured_primary_gesture(
    panel: &mut Panel,
    pointer: &PointerContext<'_>,
    selection_drag: &mut TerminalSelectionDragState,
    pos: Pos2,
    event_index: usize,
) {
    let Some(CapturedPrimaryGesture::Pty {
        terminal_mode,
        modifiers,
    }) = selection_drag.finish_primary_gesture(panel.id, pointer.ui_ctx.cumulative_frame_nr(), event_index)
    else {
        return;
    };
    let body = pointer.interaction.layout.body;
    if let Some(point) = grid_point_from_position(
        body,
        body.clamp(pos),
        pointer.metrics,
        pointer.visible_rows,
        pointer.visible_cols,
    ) && let Some(bytes) = input::mouse_button_report(PointerButton::Primary, false, modifiers, terminal_mode, point)
        && !bytes.is_empty()
    {
        panel.write_input(&bytes);
    }
}

fn handle_pointer_motion(
    panel: &mut Panel,
    pointer: &PointerContext<'_>,
    selection_drag: &TerminalSelectionDragState,
    pos: Pos2,
    pointer_buttons: input::PointerButtons,
    modifiers: egui::Modifiers,
    event_index: usize,
) {
    let frame = pointer.ui_ctx.cumulative_frame_nr();
    if suppress_unowned_primary_motion(selection_drag, panel.id, pointer_buttons, frame, event_index) {
        return;
    }
    let (terminal_mode, modifiers) = match selection_drag.primary_gesture_for_event(panel.id, frame, event_index) {
        Some(CapturedPrimaryGesture::Pty {
            terminal_mode,
            modifiers,
        }) if pointer_buttons.primary => (terminal_mode, modifiers),
        _ => (pointer.terminal_mode, modifiers),
    };
    let pos = transform_pos(pointer.from_global, pos);
    if pointer.interaction.layout.body.contains(pos)
        && pointer_motion_routes_to_pty_mouse(terminal_mode, pointer_buttons, modifiers)
        && let Some(point) = grid_point_from_position(
            pointer.interaction.layout.body,
            pos,
            pointer.metrics,
            pointer.visible_rows,
            pointer.visible_cols,
        )
        && let Some(bytes) = input::mouse_motion_report(pointer_buttons, modifiers, terminal_mode, point)
        && !bytes.is_empty()
    {
        panel.write_input(&bytes);
    }
}

fn suppress_unowned_primary_motion(
    selection_drag: &TerminalSelectionDragState,
    panel_id: PanelId,
    pointer_buttons: input::PointerButtons,
    frame: u64,
    event_index: usize,
) -> bool {
    pointer_buttons.primary
        && !matches!(
            selection_drag.primary_gesture_for_event(panel_id, frame, event_index),
            Some(CapturedPrimaryGesture::Pty { .. })
        )
}

fn body_layer_is_topmost_at(pointer: &PointerContext<'_>, pos: Pos2) -> bool {
    let global_pos = pointer.from_global.map_or(pos, |transform| transform.inverse() * pos);
    pointer.ui_ctx.layer_id_at(global_pos) == Some(pointer.interaction.body.layer_id)
}

fn pointer_buttons_at_frame_start(
    events: &[egui::Event],
    final_buttons: input::PointerButtons,
) -> input::PointerButtons {
    let mut buttons = final_buttons;
    for event in events.iter().rev() {
        if let egui::Event::PointerButton { button, pressed, .. } = event {
            set_pointer_button_state(&mut buttons, *button, !pressed);
        }
    }
    buttons
}

fn has_primary_release(events: &[egui::Event]) -> bool {
    events.iter().any(|event| {
        matches!(
            event,
            egui::Event::PointerButton {
                button: PointerButton::Primary,
                pressed: false,
                ..
            }
        )
    })
}

fn update_pointer_buttons_for_event(buttons: &mut input::PointerButtons, event: &egui::Event) {
    if let egui::Event::PointerButton { button, pressed, .. } = event {
        set_pointer_button_state(buttons, *button, *pressed);
    }
}

fn set_pointer_button_state(buttons: &mut input::PointerButtons, button: PointerButton, pressed: bool) {
    match button {
        PointerButton::Primary => buttons.primary = pressed,
        PointerButton::Middle => buttons.middle = pressed,
        PointerButton::Secondary => buttons.secondary = pressed,
        PointerButton::Extra1 | PointerButton::Extra2 => {}
    }
}

fn modifiers_for_event(event: &egui::Event) -> Option<egui::Modifiers> {
    match event {
        egui::Event::Key { modifiers, .. }
        | egui::Event::PointerButton { modifiers, .. }
        | egui::Event::MouseWheel { modifiers, .. } => Some(*modifiers),
        _ => None,
    }
}

fn handle_terminal_body_pointer_actions(
    panel: &mut Panel,
    pointer: &PointerContext<'_>,
    frame_events: &PointerFrameEvents,
    selection_drag: &mut TerminalSelectionDragState,
    selection_drag_threshold: f32,
) -> SelectionFrameOutcome {
    let mut outcome = SelectionFrameOutcome::default();
    let body_pointer_pos = pointer
        .active_pointer_pos
        .or_else(|| response_pointer_pos(&pointer.interaction.body));

    if frame_events
        .body_middle_press_pos
        .is_some_and(|pos| body_layer_is_topmost_at(pointer, pos))
        && !pty_mouse_reporting_enabled(pointer.terminal_mode, pointer.current_modifiers)
        && should_request_primary_paste(PointerButton::Middle, true, pointer.current_modifiers)
    {
        pointer
            .primary_selection
            .request_paste(panel.id, pointer.ui_ctx.clone());
        return outcome;
    }

    if let Some(pos) = frame_events.body_primary_press_pos
        && body_layer_is_topmost_at(pointer, pos)
    {
        outcome.osc8_replay_index = frame_events.osc8_replay_index;
        pointer.interaction.body.request_focus();
        start_local_selection_at(panel, pointer, pos);
        selection_drag.start(panel.id, pos);
    }

    // Collection only retains a release that follows the last local press.
    // A later outside press cannot suppress that completion, while a later
    // in-body press correctly restarts the drag.
    outcome.release_completes_drag =
        selection_drag.active_for(panel.id) && frame_events.primary_release_index.is_some();
    outcome.release_replay_index = outcome
        .release_completes_drag
        .then_some(frame_events.primary_release_index)
        .flatten();

    if selection_drag.active_for(panel.id)
        && !outcome.release_completes_drag
        && pointer.pointer_buttons.primary
        && panel.terminal().is_some_and(horizon_core::Terminal::has_selection)
        && let Some(pos) = body_pointer_pos
    {
        selection_drag.mark_dragged(panel.id, pos, selection_drag_threshold);
        update_active_selection_drag(panel, pointer, selection_drag, pos, true);
    }

    if selection_drag.active_for(panel.id)
        && outcome.release_completes_drag
        && panel.terminal().is_some_and(horizon_core::Terminal::has_selection)
        && let Some(pos) = frame_events.primary_release_pos
    {
        selection_drag.mark_dragged(panel.id, pos, selection_drag_threshold);
        update_active_selection_drag(panel, pointer, selection_drag, pos, false);
    }

    if selection_drag.active_for(panel.id) && (outcome.release_completes_drag || !pointer.pointer_buttons.primary) {
        outcome.copy_completed_selection = selection_drag.finish(panel.id);
    }

    outcome
}

fn update_active_selection_drag(
    panel: &mut Panel,
    pointer: &PointerContext<'_>,
    selection_drag: &mut TerminalSelectionDragState,
    pos: Pos2,
    schedule_continuation: bool,
) {
    let body_rect = pointer.interaction.layout.body;
    let outside_body = pos.y < body_rect.min.y || pos.y > body_rect.max.y;
    if !outside_body {
        selection_drag.clear_auto_scroll_cadence(panel.id);
        update_pointer_selection_endpoint(
            panel,
            pos,
            body_rect,
            pointer.metrics,
            pointer.visible_rows,
            pointer.visible_cols,
        );
        return;
    }

    if let Some(AutoScrollCadence::Waiting(wait)) = selection_drag.auto_scroll_cadence(panel.id, pointer.frame_time) {
        update_pointer_selection_endpoint(
            panel,
            pos,
            body_rect,
            pointer.metrics,
            pointer.visible_rows,
            pointer.visible_cols,
        );
        if schedule_continuation {
            pointer.ui_ctx.request_repaint_after(wait);
        }
        return;
    }

    let auto_scrolled = handle_pointer_selection_drag(
        panel,
        pos,
        body_rect,
        pointer.metrics,
        pointer.visible_rows,
        pointer.visible_cols,
    );
    if auto_scrolled {
        selection_drag.record_auto_scroll(panel.id, pointer.frame_time, SELECTION_AUTO_SCROLL_INTERVAL);
        if schedule_continuation {
            pointer.ui_ctx.request_repaint_after(SELECTION_AUTO_SCROLL_INTERVAL);
        }
    } else {
        selection_drag.clear_auto_scroll_cadence(panel.id);
    }
}

fn handle_pointer_button(
    panel: &mut Panel,
    pointer: &PointerContext<'_>,
    pos: Pos2,
    button: egui::PointerButton,
    pressed: bool,
    modifiers: egui::Modifiers,
) -> Option<CapturedPrimaryGesture> {
    if pressed {
        pointer.interaction.body.request_focus();
    }
    // Ctrl+click / Cmd+click opens URLs, file paths, and OSC 8 hyperlinks
    // regardless of mouse mode.
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
        open_terminal_target(&target);
        return Some(CapturedPrimaryGesture::Horizon);
    }

    if pressed
        && pointer_button_opens_osc8_hyperlink(button, modifiers)
        && let Some(uri) = osc8_hyperlink_at(panel, pointer, pos)
    {
        open_terminal_target(&uri);
        return Some(CapturedPrimaryGesture::Horizon);
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
        if button == PointerButton::Primary && pressed {
            return Some(CapturedPrimaryGesture::Pty {
                terminal_mode: pointer.terminal_mode,
                modifiers,
            });
        }
    }
    None
}

fn open_terminal_target(target: &str) {
    if let Err(error) = horizon_core::open_url(target) {
        tracing::warn!(%error, %target, "failed to open terminal target");
    }
}

fn replay_osc8_press(
    panel: &mut Panel,
    pointer: &PointerContext<'_>,
    selection_drag: &mut TerminalSelectionDragState,
    pos: Pos2,
    modifiers: egui::Modifiers,
    event_index: usize,
) -> bool {
    if !body_layer_is_topmost_at(pointer, pos)
        || !pointer_button_opens_osc8_hyperlink(PointerButton::Primary, modifiers)
    {
        return false;
    }
    let Some(uri) = osc8_hyperlink_at(panel, pointer, pos) else {
        return false;
    };
    open_terminal_target(&uri);
    let _ = selection_drag.finish(panel.id);
    if let Some(terminal) = panel.terminal() {
        terminal.clear_selection();
    }
    selection_drag.capture_primary_gesture(
        panel.id,
        CapturedPrimaryGesture::Horizon,
        pointer.ui_ctx.cumulative_frame_nr(),
        event_index,
    );
    true
}

fn osc8_hyperlink_at(panel: &Panel, pointer: &PointerContext<'_>, pos: Pos2) -> Option<String> {
    let point = grid_point_from_position(
        pointer.interaction.layout.body,
        pos,
        pointer.metrics,
        pointer.visible_rows,
        pointer.visible_cols,
    )?;
    panel.terminal()?.hyperlink_at_point(point.line, point.column)
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
    use super::{
        pointer_buttons_at_frame_start, selection_copy_completed, should_request_primary_paste,
        update_pointer_buttons_for_event,
    };
    use crate::input::PointerButtons;
    use egui::{Event, Modifiers, PointerButton, Pos2};

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

    #[test]
    fn event_order_clears_primary_before_move_that_precedes_later_press() {
        let position = Pos2::new(4.0, 4.0);
        let events = [
            Event::PointerButton {
                pos: position,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            },
            Event::PointerMoved(position),
            Event::PointerButton {
                pos: position,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::ALT,
            },
        ];
        let mut buttons = pointer_buttons_at_frame_start(
            &events,
            PointerButtons {
                primary: true,
                ..PointerButtons::default()
            },
        );
        assert!(buttons.primary);

        update_pointer_buttons_for_event(&mut buttons, &events[0]);
        assert!(
            !buttons.primary,
            "the move between release and press must see the button up"
        );
        update_pointer_buttons_for_event(&mut buttons, &events[1]);
        assert!(!buttons.primary);
        update_pointer_buttons_for_event(&mut buttons, &events[2]);
        assert!(buttons.primary);
    }
}
