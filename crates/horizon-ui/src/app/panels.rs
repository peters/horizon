use egui::{Align, Color32, Context, Id, Layout, Order, Pos2, Rect, Sense, UiBuilder, Vec2};
use horizon_core::{
    AgentSessionBinding, AgentStatus, AppShortcuts, Panel, PanelId, PanelKind, ShortcutBinding, SshConnectionStatus,
    WorkspaceId,
};

use super::super::editor_widget::{MarkdownEditorView, MarkdownPreviewCache};
use super::super::input::TerminalInputEvent;
use super::super::primary_selection::PrimarySelection;
use super::super::terminal_widget::{
    PanelNoticeAction, TerminalGridCache, TerminalKeyboardContext, TerminalSelectionDragState, TerminalView,
};
use super::super::theme;
pub(super) use super::panel_chrome::{
    PanelChrome, paint_panel_chrome, panel_kind_icon, panel_title_content_rect, show_inline_rename_editor,
};
use super::shortcut_inventory::{global_shortcut_bindings, ssh_reconnect_shortcut_conflicts};
use super::speech::SpeechWindow;
use super::view::canvas_scene_transform;
use super::{HorizonApp, PANEL_PADDING, PANEL_TITLEBAR_HEIGHT, RenameEditAction};
use crate::block_widget::CardChip;

mod resize;

pub(in crate::app) mod mic;

mod interaction;
pub(super) use interaction::ArrangedPanelDrag;
mod session_deletion;
pub(super) use session_deletion::SavedSessionDeletionState;
mod session_rebind;
#[cfg(test)]
use session_rebind::SessionRebindRenderOutcome;
use session_rebind::open_session_picker;
#[cfg(test)]
use session_rebind::render_session_rebind_options;
pub(super) use session_rebind::{session_picker_panel, session_picker_panels};

#[derive(Clone, Copy)]
pub(in crate::app) struct PanelScreenGeometry {
    pub(in crate::app) screen_rect: Rect,
    pub(in crate::app) terminal_body_screen_rect: Option<Rect>,
}

struct PanelSnapshot {
    screen_rect: Rect,
    terminal_body_screen_rect: Option<Rect>,
    canvas_position: Pos2,
    canvas_size: Vec2,
    current_workspace_id: WorkspaceId,
    kind: PanelKind,
    agent_status: AgentStatus,
    history_size: usize,
    scrollback_limit: usize,
    workspace_accent: Option<Color32>,
    is_focused: bool,
    is_renaming: bool,
    ssh_status: Option<SshConnectionStatus>,
    /// The attempt that a reconnect of this panel reached, while its mark
    /// waits for the next one.
    ssh_status_attempt: Option<u32>,
}

#[derive(Default)]
struct PanelDragOutcome {
    started: bool,
    delta: Vec2,
    stopped: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum PanelFocusRequest {
    #[default]
    None,
    Focus,
}

#[derive(Default)]
struct PanelUiOutcome {
    focus: PanelFocusRequest,
    drag: PanelDragOutcome,
    resize_delta: Vec2,
    commit_terminal_resize: bool,
    workspace_assignment: Option<WorkspaceId>,
    duplicate_browser: bool,
    /// Open a private browser panel with the page of this one.
    private_browser: bool,
    session_rebind_and_restart: Option<AgentSessionBinding>,
    command: Option<PanelCommand>,
    rename_action: RenameEditAction,
    /// A press on the microphone icon of this panel.
    mic_clicked: bool,
}

impl PanelUiOutcome {
    fn request_focus(&mut self) {
        if self.focus == PanelFocusRequest::None {
            self.focus = PanelFocusRequest::Focus;
        }
    }
}

#[derive(Clone, Copy)]
enum PanelCommand {
    Close,
    CreateWorkspace,
    StartRename,
}

struct PanelFrame {
    panel: Rect,
    titlebar: Rect,
    body: Rect,
    close: Rect,
    /// The square of the microphone icon, left of the close control. It shows an
    /// icon only on a panel that speech input can enter.
    mic: Rect,
}

impl PanelFrame {
    fn new(panel_rect: Rect) -> Self {
        let titlebar = Rect::from_min_max(
            panel_rect.min,
            Pos2::new(panel_rect.max.x, panel_rect.min.y + PANEL_TITLEBAR_HEIGHT),
        );
        let body = Rect::from_min_max(
            Pos2::new(panel_rect.min.x + PANEL_PADDING, titlebar.max.y + PANEL_PADDING),
            Pos2::new(panel_rect.max.x - PANEL_PADDING, panel_rect.max.y - PANEL_PADDING),
        );
        let close = Rect::from_center_size(
            Pos2::new(panel_rect.max.x - 18.0, panel_rect.min.y + PANEL_TITLEBAR_HEIGHT * 0.5),
            Vec2::splat(16.0),
        );
        let mic = mic::mic_rect(panel_rect, titlebar);
        Self {
            panel: panel_rect,
            titlebar,
            body,
            close,
            mic,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct PanelRenderScope {
    /// The window that draws the panel, for the take of speech input that its icon
    /// starts and stops.
    pub(super) window: SpeechWindow,
    /// Detached viewports dispatch only the fit/minimap toolbar shortcuts.
    pub(super) detached: bool,
    pub(super) host_dialog_open: bool,
    /// The viewport's event slice carries a pointer-button press this frame.
    pub(super) frame_has_pointer_button: bool,
}

struct PanelBodyContext<'a> {
    keyboard_events: &'a [TerminalInputEvent],
    browser_events: &'a [egui::Event],
    editor_save_shortcut: ShortcutBinding,
    editor_preview_cache: Option<&'a mut MarkdownPreviewCache>,
    local_ssh_reconnect_enabled: bool,
    primary_selection: &'a PrimarySelection,
    clipboard_images: &'a crate::clipboard_image::ClipboardImages,
    /// What the person pressed in the line under the grid of this panel.
    notice_action: &'a mut PanelNoticeAction,
    /// The person created or revealed this panel, so its body takes the keyboard.
    take_keyboard_focus: bool,
    terminal_selection_drag: &'a mut TerminalSelectionDragState,
    terminal_grid_cache: Option<&'a mut TerminalGridCache>,
    browser_ui_state: Option<&'a mut crate::browser_widget::BrowserUiState>,
    device_ui_state: Option<&'a mut crate::device_widget::DeviceUiState>,
    /// The clicks and row selection of a card panel.
    card_runtime: Option<&'a mut horizon_core::CardRuntime>,
    /// The address of the link a card's person pressed in this pass.
    card_opened_link: &'a mut Option<String>,
    /// The id of the button a person pressed in a card that Horizon owns.
    card_pressed: &'a mut Option<String>,
    /// The person pressed the mark of a card that the view cuts.
    card_reveal: &'a mut bool,
    /// An agent panel of the card's owner is in its workspace.
    card_agent_present: bool,
    /// Horizon wrote the document of this card, so no agent reads its presses.
    card_host_owned: bool,
    /// Display name of the agent that owns the card, while it is present.
    card_owner_name: Option<&'a str>,
    browser_shortcuts: Option<&'a AppShortcuts>,
    browser_shortcut_bindings: &'a [ShortcutBinding],
    browser_frame_has_pointer_button: bool,
    browser_fullscreen_active: bool,
    browser_zoom_wheel_owner: crate::browser_widget::ZoomWheelOwner,
    /// Canvas points at the bottom of the body that the terminal scrollbar
    /// leaves to a corner resize grip; zero when the panel has no grip.
    terminal_scrollbar_bottom_clearance: f32,
}

fn show_panel_body_contents(
    ui: &mut egui::Ui,
    panel: &mut Panel,
    is_focused: bool,
    interactive: bool,
    body_context: PanelBodyContext<'_>,
) -> bool {
    match panel.kind {
        PanelKind::Editor => MarkdownEditorView::new(panel, body_context.editor_preview_cache)
            .show(ui, body_context.editor_save_shortcut),
        PanelKind::Device if panel.device().is_some() => {
            if let (Some(state), Some(device)) = (body_context.device_ui_state, panel.device()) {
                state.show(ui, device, interactive);
            }
            false
        }
        PanelKind::Block => {
            let (Some(block), Some(card)) = (panel.block(), body_context.card_runtime) else {
                ui.centered_and_justified(|ui| ui.label("Card state unavailable"));
                return false;
            };
            let opened_link = body_context.card_opened_link;
            crate::block_widget::show(
                ui,
                panel.id,
                crate::block_widget::CardView {
                    document: &block.document,
                    card,
                    revision: block.revision,
                    agent_present: body_context.card_agent_present,
                    host_owned: body_context.card_host_owned,
                    interactive,
                    now_millis: horizon_browser_control::manifest::now_millis(),
                    owner_name: body_context.card_owner_name,
                    owner_session: block
                        .owner_actor
                        .as_deref()
                        .and_then(horizon_core::node_cards::actor_parts),
                    updated_at_millis: block.updated_at_millis,
                    opened_link,
                    pressed: body_context.card_pressed,
                    reveal: body_context.card_reveal,
                },
            );
            false
        }
        PanelKind::Browser => {
            let (Some(state), Some(shortcuts)) = (body_context.browser_ui_state, body_context.browser_shortcuts) else {
                ui.centered_and_justified(|ui| ui.label("Browser state unavailable"));
                return false;
            };
            crate::browser_widget::BrowserView::new(
                panel,
                state,
                shortcuts,
                body_context.browser_fullscreen_active,
                body_context.browser_zoom_wheel_owner,
                body_context.browser_shortcut_bindings,
                body_context.browser_frame_has_pointer_button,
            )
            .show(ui, body_context.browser_events, is_focused, interactive)
        }
        _ => TerminalView::new(
            panel,
            body_context.terminal_grid_cache,
            body_context.terminal_scrollbar_bottom_clearance,
        )
        .show(
            ui,
            is_focused,
            interactive,
            body_context.terminal_selection_drag,
            TerminalKeyboardContext {
                keyboard_events: body_context.keyboard_events,
                primary_selection: body_context.primary_selection,
                clipboard_images: body_context.clipboard_images,
                local_ssh_reconnect_enabled: body_context.local_ssh_reconnect_enabled,
                notice_action: body_context.notice_action,
                take_keyboard_focus: body_context.take_keyboard_focus,
            },
        ),
    }
}

fn clip_screen_rect_to_canvas(raw_rect: Rect, canvas_rect: Rect) -> Option<Rect> {
    let clipped = raw_rect.intersect(canvas_rect);
    (clipped.is_positive()
        && clipped.min.x.is_finite()
        && clipped.min.y.is_finite()
        && clipped.max.x.is_finite()
        && clipped.max.y.is_finite())
    .then_some(clipped)
}

trait CanvasClippedArea {
    fn clip_to_canvas(self, canvas_layer_clip: Rect) -> Self;
}

impl CanvasClippedArea for egui::Area {
    fn clip_to_canvas(self, canvas_layer_clip: Rect) -> Self {
        // Keep the explicit clip for hit-testing without forcing the panel's
        // freely positioned canvas coordinates inside that rectangle.
        self.constrain_to(canvas_layer_clip).constrain(false)
    }
}

impl HorizonApp {
    /// Runs what the person pressed in the line under the grid of `panel_id`.
    ///
    /// A restart and a close wait for the end of the frame: the restart asks the
    /// coordination worker for its pre-flight, and the board must not lose a
    /// panel that this pass still draws. A reconnect is the one request that the
    /// frame makes itself, as it did before the line offered other controls.
    pub(in crate::app) fn run_panel_notice_action(&mut self, panel_id: PanelId, action: PanelNoticeAction) {
        match action {
            PanelNoticeAction::None => {}
            PanelNoticeAction::Reconnect => self.reconnect_panel_now(panel_id),
            PanelNoticeAction::StartProgramAgain => self.queue_panel_restart(panel_id),
            PanelNoticeAction::ClosePanel => {
                if !self.panels_to_close.contains(&panel_id) {
                    self.panels_to_close.push(panel_id);
                }
            }
        }
    }

    fn local_ssh_reconnect_shortcut_enabled(&self) -> bool {
        !ssh_reconnect_shortcut_conflicts(&self.shortcuts)
    }

    pub(in crate::app) fn visible_panel_geometry_for_canvas_view(
        &self,
        canvas_rect: Rect,
        visible_workspace: Option<WorkspaceId>,
    ) -> Vec<(PanelId, PanelScreenGeometry)> {
        self.board
            .panels
            .iter()
            .filter(|panel| panel.visible)
            .filter(|panel| match visible_workspace {
                Some(workspace_id) => panel.workspace_id == workspace_id,
                None => !self.workspace_is_detached(panel.workspace_id),
            })
            .filter_map(|panel| {
                self.panel_screen_geometry(panel, canvas_rect)
                    .map(|geometry| (panel.id, geometry))
            })
            .collect()
    }

    /// The rectangle that a panel takes on the canvas, before the canvas clips it.
    fn panel_canvas_rect(&self, panel: &Panel) -> Rect {
        let position = self.arranged_panel_position(
            panel.id,
            panel.workspace_id,
            Pos2::new(panel.layout.position[0], panel.layout.position[1]),
        );
        Rect::from_min_size(position, Vec2::new(panel.layout.size[0], panel.layout.size[1]))
    }

    /// Where a panel stands on the screen, whether or not the canvas shows it.
    ///
    /// A line that an agent draws to a panel keeps the edge of that panel, and not
    /// the edge of the canvas, when the panel is only partly on the screen. The
    /// painter of the line clips the line to the canvas.
    pub(in crate::app) fn panel_screen_place(&self, panel: &Panel, canvas_rect: Rect) -> Rect {
        let place = self.panel_canvas_rect(panel);
        Rect::from_min_size(
            self.canvas_to_screen(canvas_rect, place.min),
            self.canvas_size_to_screen(place.size()),
        )
    }

    pub(super) fn panel_screen_geometry(&self, panel: &Panel, canvas_rect: Rect) -> Option<PanelScreenGeometry> {
        let canvas_rect_of_panel = self.panel_canvas_rect(panel);
        let screen_rect = clip_screen_rect_to_canvas(self.panel_screen_place(panel, canvas_rect), canvas_rect)?;
        let has_body = panel.terminal().is_some();
        let terminal_body_screen_rect = has_body.then_some(()).and_then(|()| {
            let body_rect = PanelFrame::new(canvas_rect_of_panel).body;
            clip_screen_rect_to_canvas(
                Rect::from_min_size(
                    self.canvas_to_screen(canvas_rect, body_rect.min),
                    self.canvas_size_to_screen(body_rect.size()),
                ),
                canvas_rect,
            )
        });

        Some(PanelScreenGeometry {
            screen_rect,
            terminal_body_screen_rect,
        })
    }

    #[profiling::function]
    pub(super) fn render_fullscreen_panel(&mut self, ui: &mut egui::Ui) {
        let Some(panel_id) = self.fullscreen_panel else {
            return;
        };
        if !self.board.panel(panel_id).is_some_and(|panel| panel.visible) {
            self.fullscreen_panel = None;
            return;
        }
        let raw_events = ui.ctx().input(|input| input.events.clone());
        self.terminal_keyboard_events = self.terminal_events_for_viewport(ui.ctx(), &raw_events);
        let local_ssh_reconnect_enabled = self.local_ssh_reconnect_shortcut_enabled();
        let browser_shortcuts = self
            .board
            .panel(panel_id)
            .is_some_and(|panel| panel.kind == PanelKind::Browser)
            .then(|| self.shortcuts.clone());
        let browser_events = browser_shortcuts.as_ref().map_or_else(Vec::new, |_| {
            self.terminal_keyboard_events
                .iter()
                .map(|input| input.event.clone())
                .collect()
        });
        let frame_has_pointer_button = browser_events
            .iter()
            .any(|event| matches!(event, egui::Event::PointerButton { .. }));
        let all_shortcut_bindings = global_shortcut_bindings(&self.shortcuts);

        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(theme::PANEL_BG()))
            .show(ui, |ui| {
                let rect = ui.available_rect_before_wrap();
                let body_rect = Rect::from_min_max(
                    Pos2::new(rect.min.x + PANEL_PADDING, rect.min.y + PANEL_PADDING),
                    Pos2::new(rect.max.x - PANEL_PADDING, rect.max.y - PANEL_PADDING),
                );

                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(body_rect)
                        .layout(Layout::top_down(Align::Min)),
                    |ui| {
                        let mut notice_action = PanelNoticeAction::None;
                        let mut card_opened_link = None;
                        let mut card_pressed = None;
                        let mut card_reveal = false;
                        let claim_editor_focus = true;
                        let take_keyboard_focus = self.keyboard_focus_requested(panel_id);
                        let interactive = !self.host_dialog_open() && session_picker_panel(ui.ctx()).is_none();
                        let card_agent_present = self.card_agent_present(panel_id);
                        let card_host_owned = self.card_host_owned(panel_id);
                        let card_owner_name = self.card_owner_name(panel_id);
                        if let Some(panel) = self.board.panel_mut(panel_id) {
                            let preview_cache = if panel.kind == PanelKind::Editor {
                                Some(
                                    self.panel_render_caches
                                        .editor_preview_cache
                                        .entry(panel_id)
                                        .or_default(),
                                )
                            } else {
                                None
                            };
                            let browser_state = if panel.kind == PanelKind::Browser {
                                Some(self.panel_render_caches.browser_ui_state.entry(panel_id).or_default())
                            } else {
                                None
                            };
                            let device_state = (panel.kind == PanelKind::Device)
                                .then(|| self.panel_render_caches.device_ui_state.entry(panel_id).or_default());
                            let card_runtime = (panel.kind == PanelKind::Block)
                                .then(|| self.panel_render_caches.card_runtime.entry(panel_id).or_default());
                            show_panel_body_contents(
                                ui,
                                panel,
                                claim_editor_focus,
                                interactive,
                                PanelBodyContext {
                                    keyboard_events: &self.terminal_keyboard_events,
                                    browser_events: &browser_events,
                                    editor_save_shortcut: self.shortcuts.save_editor,
                                    editor_preview_cache: preview_cache,
                                    local_ssh_reconnect_enabled,
                                    primary_selection: &self.primary_selection,
                                    clipboard_images: &self.clipboard_images,
                                    notice_action: &mut notice_action,
                                    take_keyboard_focus,
                                    terminal_selection_drag: &mut self.terminal_selection_drag,
                                    terminal_grid_cache: None,
                                    browser_ui_state: browser_state,
                                    device_ui_state: device_state,
                                    card_runtime,
                                    card_opened_link: &mut card_opened_link,
                                    card_pressed: &mut card_pressed,
                                    card_reveal: &mut card_reveal,
                                    card_agent_present,
                                    card_host_owned,
                                    card_owner_name,
                                    browser_shortcuts: browser_shortcuts.as_ref(),
                                    browser_shortcut_bindings: &all_shortcut_bindings,
                                    browser_frame_has_pointer_button: frame_has_pointer_button,
                                    browser_fullscreen_active: true,
                                    browser_zoom_wheel_owner: crate::browser_widget::ZoomWheelOwner::Page,
                                    terminal_scrollbar_bottom_clearance: 0.0,
                                },
                            );
                        }
                        self.run_panel_notice_action(panel_id, notice_action);
                        if take_keyboard_focus {
                            self.keyboard_focus_answered(panel_id);
                        }
                        if let Some(url) = card_opened_link {
                            self.open_card_link(ui.ctx(), panel_id, &url);
                        }
                        if let Some(id) = card_pressed {
                            self.press_host_card_button(ui.ctx(), panel_id, &id);
                        }
                        if card_reveal {
                            self.reveal_panel_visible(ui.ctx(), panel_id);
                        }
                        self.paint_speech_mark(&ui.painter_at(rect), panel_id, rect);
                    },
                );
            });
    }

    #[profiling::function]
    pub(super) fn render_panels(&mut self, ctx: &Context) {
        self.capture_root_device_presentation(self.canvas_rect(ctx));
        self.panel_screen_rects.clear();
        self.terminal_body_screen_rects.clear();
        self.panel_screen_order.clear();
        let workspace_collision_ids = self.workspace_collision_scope(None);

        // Reuse workspace color vec across frames (avoids per-frame String
        // clones — names are looked up lazily in the context menu).
        self.workspace_colors.clear();
        self.workspace_colors.extend(
            self.board
                .workspaces
                .iter()
                .map(|workspace| (workspace.id, theme::workspace_accent(workspace.color_idx))),
        );

        // Reuse panel ordering vec across frames. Collect into a local first,
        // then swap into the field, because the filter borrows self immutably
        // while extend borrows panel_render_order mutably.
        let mut order = std::mem::take(&mut self.panel_render_order);
        order.clear();
        order.extend(
            self.board
                .panels
                .iter()
                .filter(|panel| panel.visible && !self.workspace_is_detached(panel.workspace_id))
                .enumerate()
                .map(|(index, panel)| (panel.id, index)),
        );
        self.panel_render_order = order;
        let focused = self.board.focused;
        self.panel_render_order
            .sort_by_key(|(panel_id, _)| Some(*panel_id) == focused);

        let canvas_rect = self.canvas_rect(ctx);
        let has_browser = self.panel_render_order.iter().any(|(panel_id, _)| {
            self.board
                .panel(*panel_id)
                .is_some_and(|panel| panel.kind == PanelKind::Browser)
        });
        let browser_events = if has_browser {
            self.terminal_keyboard_events
                .iter()
                .map(|input| input.event.clone())
                .collect()
        } else {
            Vec::new()
        };
        let frame_has_pointer_button = browser_events
            .iter()
            .any(|event| matches!(event, egui::Event::PointerButton { .. }));
        let mut panels_to_close = Vec::new();
        let host_dialog_open = self.host_dialog_open();

        for i in 0..self.panel_render_order.len() {
            let (panel_id, _fallback_index) = self.panel_render_order[i];
            if self.render_panel(
                ctx,
                canvas_rect,
                panel_id,
                &workspace_collision_ids,
                &browser_events,
                PanelRenderScope {
                    window: SpeechWindow::Root,
                    detached: false,
                    host_dialog_open,
                    frame_has_pointer_button,
                },
            ) {
                panels_to_close.push(panel_id);
            }
        }

        self.panels_to_close = panels_to_close;
        // The mark of a take is painted after every panel, so the body of its panel
        // cannot cover it and no panel pays for it on a frame without a take.
        if let Some(target) = self.speech_target_on_screen() {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::unique("speech_state_mark"),
            ));
            self.paint_speech_mark(&painter, target, self.panel_screen_rects[&target]);
        }
    }

    /// The panel of the take of speech input, when this frame drew it and the
    /// Settings panel does not cover it.
    fn speech_target_on_screen(&self) -> Option<PanelId> {
        let target = self.speech.target()?;
        let rect = *self.panel_screen_rects.get(&target)?;
        let covered = self
            .settings
            .as_ref()
            .and_then(|editor| editor.panel_rect)
            .is_some_and(|panel| panel.intersects(rect));
        (!covered).then_some(target)
    }

    #[profiling::function]
    pub(super) fn render_panel(
        &mut self,
        ctx: &Context,
        canvas_rect: Rect,
        panel_id: PanelId,
        workspace_collision_ids: &[WorkspaceId],
        browser_events: &[egui::Event],
        scope: PanelRenderScope,
    ) -> bool {
        self.clear_inactive_arranged_panel_drag(ctx, panel_id);
        let Some(snapshot) = self.panel_snapshot(panel_id, canvas_rect) else {
            return false;
        };
        let outcome = self.show_panel_area(ctx, canvas_rect, panel_id, &snapshot, browser_events, scope);
        self.apply_panel_outcome(
            ctx,
            panel_id,
            &snapshot,
            &outcome,
            scope.window,
            workspace_collision_ids,
        )
    }

    #[profiling::function]
    fn panel_snapshot(&self, panel_id: PanelId, canvas_rect: Rect) -> Option<PanelSnapshot> {
        self.board
            .panel(panel_id)
            .filter(|panel| panel.visible)
            .and_then(|panel| {
                let geometry = self.panel_screen_geometry(panel, canvas_rect)?;
                let terminal = panel.terminal();
                let canvas_position = self.arranged_panel_position(
                    panel.id,
                    panel.workspace_id,
                    Pos2::new(panel.layout.position[0], panel.layout.position[1]),
                );
                let canvas_size = Vec2::new(panel.layout.size[0], panel.layout.size[1]);

                let workspace_accent = self
                    .workspace_colors
                    .iter()
                    .find(|(workspace_id, _)| *workspace_id == panel.workspace_id)
                    .map(|(_, color)| *color);

                Some(PanelSnapshot {
                    screen_rect: geometry.screen_rect,
                    terminal_body_screen_rect: geometry.terminal_body_screen_rect,
                    canvas_position,
                    canvas_size,
                    current_workspace_id: panel.workspace_id,
                    kind: panel.kind,
                    agent_status: panel.agent_status(),
                    history_size: terminal.map_or(0, horizon_core::Terminal::history_size),
                    scrollback_limit: terminal.map_or(0, horizon_core::Terminal::scrollback_limit),
                    workspace_accent,
                    is_focused: self.board.focused == Some(panel_id),
                    is_renaming: self.renaming_panel == Some(panel_id),
                    ssh_status: panel.ssh_status(),
                    ssh_status_attempt: panel.ssh_status_attempt(),
                })
            })
    }

    #[profiling::function]
    fn show_panel_area(
        &mut self,
        ctx: &Context,
        canvas_rect: Rect,
        panel_id: PanelId,
        snapshot: &PanelSnapshot,
        browser_events: &[egui::Event],
        scope: PanelRenderScope,
    ) -> PanelUiOutcome {
        let mut outcome = PanelUiOutcome::default();
        let interactive =
            !self.canvas_pan_input_claimed && !scope.host_dialog_open && session_picker_panel(ctx).is_none();
        let browser_canvas_zoom_active = true;
        let local_ssh_reconnect_enabled = self.local_ssh_reconnect_shortcut_enabled();
        let browser_shortcuts = (snapshot.kind == PanelKind::Browser).then(|| self.shortcuts.clone());
        // Browser input suppresses app shortcuts that this viewport actually
        // handles: the root window routes the full global set, detached
        // windows only the two toolbar shortcuts.
        let all_shortcut_bindings = global_shortcut_bindings(&self.shortcuts);
        let detached_shortcut_bindings = [self.shortcuts.fit_active_workspace, self.shortcuts.toggle_minimap];
        let browser_shortcut_bindings = if scope.detached {
            &detached_shortcut_bindings[..]
        } else {
            &all_shortcut_bindings[..]
        };
        // Area registers its parent response before the closure applies the
        // canvas transform and clip. Give that response the same local-layer
        // clip so an off-canvas panel cannot block fixed overlay controls.
        let canvas_layer_clip = canvas_scene_transform(canvas_rect, self.canvas_view).inverse() * canvas_rect;

        // The area must stay interactable: since egui 0.36, non-interactable
        // layers are skipped by hit-testing, which makes every widget inside
        // (titlebar drag, close, resize handles) click-through. It is not
        // interactable where the Settings pane holds the pointer, so the layer of
        // the pane takes the wheel and the presses there, and it stays
        // interactable while a button is held, so a drag that passes over the pane
        // keeps moving its panel until the release.
        let pane_owns_pointer = !scope.detached
            && self
                .settings
                .as_ref()
                .and_then(|editor| editor.panel_rect)
                .is_some_and(|pane| {
                    ctx.input(|input| {
                        input
                            .pointer
                            .interact_pos()
                            .or_else(|| input.pointer.hover_pos())
                            .is_some_and(|position| pane.contains(position))
                            && !input.pointer.any_down()
                            && !input.pointer.any_released()
                    })
                });
        egui::Area::new(Id::unique(("panel", panel_id.0)))
            .fixed_pos(snapshot.canvas_position)
            .clip_to_canvas(canvas_layer_clip)
            .order(if snapshot.is_focused {
                Order::Foreground
            } else {
                Order::Middle
            })
            .interactable(!pane_owns_pointer)
            .show(ctx, |ui| {
                self.apply_canvas_layer_transform(ui, canvas_rect);
                let (panel_rect, _) = ui.allocate_exact_size(snapshot.canvas_size, Sense::hover());
                let rects = PanelFrame::new(panel_rect);
                let drag_response = ui.interact(
                    rects.titlebar,
                    ui.make_persistent_id(("panel_drag", panel_id.0)),
                    if !interactive || snapshot.is_renaming {
                        Sense::hover()
                    } else {
                        Sense::click_and_drag()
                    },
                );
                let close_response = ui.interact(
                    rects.close.expand2(Vec2::splat(4.0)),
                    ui.make_persistent_id(("panel_close", panel_id.0)),
                    if interactive { Sense::click() } else { Sense::hover() },
                );
                // The icon registers after the drag area and the close control, so a
                // press on it belongs to the take and never to a drag or a close.
                let mic = self.panel_mic(panel_id, scope.window, interactive);
                let mic_widget = mic.map(|mic| mic::show(ui, panel_id, rects.mic, mic));
                if interactive && !snapshot.is_renaming {
                    self.show_panel_context_menu(
                        &drag_response,
                        panel_id,
                        snapshot.current_workspace_id,
                        snapshot.kind,
                        &mut outcome,
                    );
                }

                // Update the grip layer before raw body events use topmost-layer routing.
                // A card keeps the grip below its footer strip, so the strip keeps
                // every press at every canvas zoom.
                let resize_response = resize::show_resize_control(
                    ui,
                    rects.panel,
                    self.canvas_view.zoom,
                    panel_id,
                    interactive,
                    (snapshot.kind == PanelKind::Block).then_some(crate::block_widget::CARD_FOOT_SPAN),
                );

                // Compute display_title and the session badge from the board on
                // demand, avoiding a per-panel String clone in PanelSnapshot.
                // The Cow is borrowed when the underlying panel title is
                // sufficient, and only allocates when a formatted composite
                // title is needed.
                let panel = self.board.panel(panel_id);
                let display_title = if snapshot.is_renaming {
                    None
                } else {
                    panel.map(|p| p.display_title())
                };
                let session_id = panel.and_then(|p| p.session_id());
                // The mark of a terminal that an agent opened for the person,
                // or of a terminal that runs on the machine of its workspace's
                // place. A card keeps its own state chip instead.
                let opened_by = panel.and_then(Panel::opened_by);
                let on_machine = panel.and_then(Panel::machine);

                let chrome = PanelChrome {
                    panel_id,
                    kind: snapshot.kind,
                    agent_status: snapshot.agent_status,
                    panel_rect: rects.panel,
                    titlebar_rect: rects.titlebar,
                    close_rect: rects.close,
                    title: display_title.as_deref(),
                    history_size: snapshot.history_size,
                    scrollback_limit: snapshot.scrollback_limit,
                    focused: snapshot.is_focused,
                    close_hovered: close_response.hovered(),
                    workspace_accent: snapshot.workspace_accent,
                    ssh_status: snapshot.ssh_status,
                    ssh_status_attempt: snapshot.ssh_status_attempt,
                    session_id,
                    mic: mic_widget.as_ref().map(|widget| widget.control),
                    card: self
                        .card_titlebar_chip(panel_id)
                        .or_else(|| opened_by.map(CardChip::opened_by))
                        .or_else(|| on_machine.map(CardChip::on_machine)),
                };
                paint_panel_chrome(ui, chrome);

                if snapshot.is_renaming {
                    outcome.rename_action = show_inline_rename_editor(
                        ui,
                        panel_title_content_rect(&chrome),
                        &mut self.panel_rename_buffer,
                        egui::FontId::proportional(13.0),
                    );
                }

                // Release the shared board borrow before the mutable borrow below.
                drop(display_title);

                // The corner grip owns the panel's bottom-right square, so the
                // terminal scrollbar stops above it and stays reachable.
                let terminal_scrollbar_bottom_clearance =
                    resize::scrollbar_bottom_clearance(rects.panel, self.canvas_view.zoom);
                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(rects.body)
                        .layout(Layout::top_down(Align::Min)),
                    |ui| {
                        let mut notice_action = PanelNoticeAction::None;
                        let mut card_opened_link = None;
                        let mut card_pressed = None;
                        let mut card_reveal = false;
                        let claim_editor_focus = snapshot.is_focused;
                        let take_keyboard_focus = self.keyboard_focus_requested(panel_id);
                        let card_agent_present = self.card_agent_present(panel_id);
                        let card_host_owned = self.card_host_owned(panel_id);
                        let card_owner_name = self.card_owner_name(panel_id);
                        let board = &mut self.board;
                        let editor_preview_cache = &mut self.panel_render_caches.editor_preview_cache;
                        let terminal_grid_cache = &mut self.panel_render_caches.terminal_grid_cache;
                        let browser_ui_state = &mut self.panel_render_caches.browser_ui_state;
                        let device_ui_state = &mut self.panel_render_caches.device_ui_state;
                        let card_runtimes = &mut self.panel_render_caches.card_runtime;
                        let terminal_selection_drag = &mut self.terminal_selection_drag;
                        if let Some(panel) = board.panel_mut(panel_id) {
                            let preview_cache = if panel.kind == PanelKind::Editor {
                                Some(editor_preview_cache.entry(panel_id).or_default())
                            } else {
                                None
                            };
                            let grid_cache = if panel.terminal().is_some() {
                                Some(terminal_grid_cache.entry(panel_id).or_default())
                            } else {
                                None
                            };
                            let browser_state = if panel.kind == PanelKind::Browser {
                                Some(browser_ui_state.entry(panel_id).or_default())
                            } else {
                                None
                            };
                            let device_state =
                                (panel.kind == PanelKind::Device).then(|| device_ui_state.entry(panel_id).or_default());
                            let card_runtime =
                                (panel.kind == PanelKind::Block).then(|| card_runtimes.entry(panel_id).or_default());
                            if show_panel_body_contents(
                                ui,
                                panel,
                                claim_editor_focus,
                                interactive,
                                PanelBodyContext {
                                    keyboard_events: &self.terminal_keyboard_events,
                                    browser_events,
                                    editor_save_shortcut: self.shortcuts.save_editor,
                                    editor_preview_cache: preview_cache,
                                    local_ssh_reconnect_enabled,
                                    primary_selection: &self.primary_selection,
                                    clipboard_images: &self.clipboard_images,
                                    notice_action: &mut notice_action,
                                    take_keyboard_focus,
                                    terminal_selection_drag,
                                    terminal_grid_cache: grid_cache,
                                    browser_ui_state: browser_state,
                                    device_ui_state: device_state,
                                    card_runtime,
                                    card_opened_link: &mut card_opened_link,
                                    card_pressed: &mut card_pressed,
                                    card_reveal: &mut card_reveal,
                                    card_agent_present,
                                    card_host_owned,
                                    card_owner_name,
                                    browser_shortcuts: browser_shortcuts.as_ref(),
                                    browser_shortcut_bindings,
                                    browser_frame_has_pointer_button: scope.frame_has_pointer_button,
                                    browser_fullscreen_active: false,
                                    browser_zoom_wheel_owner: if browser_canvas_zoom_active {
                                        crate::browser_widget::ZoomWheelOwner::App
                                    } else {
                                        crate::browser_widget::ZoomWheelOwner::Page
                                    },
                                    terminal_scrollbar_bottom_clearance,
                                },
                            ) {
                                outcome.request_focus();
                            }
                        }
                        self.run_panel_notice_action(panel_id, notice_action);
                        if take_keyboard_focus {
                            self.keyboard_focus_answered(panel_id);
                        }
                        if let Some(url) = card_opened_link {
                            self.open_card_link(ctx, panel_id, &url);
                        }
                        if let Some(id) = card_pressed {
                            self.press_host_card_button(ctx, panel_id, &id);
                        }
                        if card_reveal {
                            self.reveal_panel_visible(ctx, panel_id);
                        }
                    },
                );
                if interactive {
                    Self::update_panel_interactions(
                        snapshot.is_renaming,
                        &drag_response,
                        &close_response,
                        mic_widget.as_ref().map(|widget| &widget.response),
                        &resize_response,
                        &mut outcome,
                    );
                }
                if interactive && (resize_response.hovered() || resize_response.dragged()) {
                    ctx.set_cursor_icon(egui::CursorIcon::ResizeNwSe);
                }
                outcome.session_rebind_and_restart = self.render_saved_session_picker(ctx, panel_id);
            });

        outcome
    }
}

#[cfg(test)]
mod tests;
