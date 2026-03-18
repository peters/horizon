use egui::{Align, Color32, Context, Id, Layout, Order, Pos2, Rect, Sense, UiBuilder, Vec2};
use horizon_core::{AttentionSeverity, Panel, PanelId, PanelKind, WorkspaceId};

use crate::editor_widget::MarkdownEditorView;
use crate::git_changes_widget::GitChangesView;
use crate::terminal_widget::{TerminalGridCache, TerminalView, viewport_for_available_space};
use crate::theme;
use crate::usage_widget::UsageDashboardView;

pub(super) use super::panel_chrome::{
    PanelChrome, paint_panel_chrome, panel_kind_icon, panel_title_content_rect, show_inline_rename_editor,
};
use super::util::clamp_panel_size;
use super::{HorizonApp, PANEL_PADDING, PANEL_TITLEBAR_HEIGHT, RESIZE_HANDLE_SIZE, RenameEditAction};

struct PanelSnapshot {
    screen_rect: Rect,
    canvas_position: Pos2,
    canvas_size: Vec2,
    current_workspace_id: WorkspaceId,
    title: String,
    display_title: String,
    kind: PanelKind,
    history_size: usize,
    scrollback_limit: usize,
    workspace_accent: Option<Color32>,
    is_focused: bool,
    is_renaming: bool,
    attention_badge: Option<(AttentionSeverity, String)>,
}

#[derive(Default)]
struct PanelUiOutcome {
    focus_requested: bool,
    drag_delta: Vec2,
    resize_delta: Vec2,
    commit_terminal_resize: bool,
    workspace_assignment: Option<WorkspaceId>,
    command: Option<PanelCommand>,
    rename_action: RenameEditAction,
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
    resize: Rect,
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
        let resize = Rect::from_min_size(
            Pos2::new(
                panel_rect.max.x - RESIZE_HANDLE_SIZE,
                panel_rect.max.y - RESIZE_HANDLE_SIZE,
            ),
            Vec2::splat(RESIZE_HANDLE_SIZE),
        );

        Self {
            panel: panel_rect,
            titlebar,
            body,
            close,
            resize,
        }
    }
}

fn show_panel_body_contents(
    ui: &mut egui::Ui,
    panel: &mut Panel,
    is_focused: bool,
    terminal_grid_cache: Option<&mut TerminalGridCache>,
) -> bool {
    match panel.kind {
        PanelKind::Editor => MarkdownEditorView::new(panel).show(ui, is_focused),
        PanelKind::GitChanges => GitChangesView::new(panel).show(ui, is_focused),
        PanelKind::Usage => UsageDashboardView::new(panel).show(ui, is_focused),
        _ => TerminalView::new(panel, terminal_grid_cache).show(ui, is_focused),
    }
}

impl HorizonApp {
    #[profiling::function]
    pub(super) fn render_fullscreen_panel(&mut self, ctx: &Context) {
        let Some(panel_id) = self.fullscreen_panel else {
            return;
        };

        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(theme::PANEL_BG))
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                let body_rect = Rect::from_min_max(
                    Pos2::new(rect.min.x + PANEL_PADDING, rect.min.y + PANEL_PADDING),
                    Pos2::new(rect.max.x - PANEL_PADDING, rect.max.y - PANEL_PADDING),
                );

                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(body_rect)
                        .layout(Layout::top_down(Align::Min)),
                    |ui| {
                        if let Some(panel) = self.board.panel_mut(panel_id) {
                            show_panel_body_contents(ui, panel, true, None);
                        }
                    },
                );
            });
    }

    #[profiling::function]
    pub(super) fn render_panels(&mut self, ctx: &Context) {
        self.panel_screen_rects.clear();

        let workspaces: Vec<(WorkspaceId, String, Color32)> = self
            .board
            .workspaces
            .iter()
            .map(|workspace| {
                let (r, g, b) = workspace.accent();
                (workspace.id, workspace.name.clone(), Color32::from_rgb(r, g, b))
            })
            .collect();

        let mut panel_order: Vec<_> = self
            .board
            .panels
            .iter()
            .filter(|panel| !self.workspace_is_detached(panel.workspace_id))
            .enumerate()
            .map(|(index, panel)| (panel.id, index))
            .collect();
        let focused = self.board.focused;
        panel_order.sort_by_key(|(panel_id, _)| Some(*panel_id) == focused);

        let canvas_rect = self.canvas_rect(ctx);
        let mut panels_to_close = Vec::new();

        for (panel_id, fallback_index) in panel_order {
            if self.render_panel(ctx, canvas_rect, panel_id, fallback_index, &workspaces) {
                panels_to_close.push(panel_id);
            }
        }

        self.panels_to_close = panels_to_close;
    }

    #[profiling::function]
    pub(super) fn render_panel(
        &mut self,
        ctx: &Context,
        canvas_rect: Rect,
        panel_id: PanelId,
        _fallback_index: usize,
        workspaces: &[(WorkspaceId, String, Color32)],
    ) -> bool {
        let Some(snapshot) = self.panel_snapshot(panel_id, canvas_rect, workspaces) else {
            return false;
        };
        let outcome = self.show_panel_area(ctx, canvas_rect, panel_id, &snapshot, workspaces);
        self.apply_panel_outcome(ctx, panel_id, &snapshot, &outcome)
    }

    #[profiling::function]
    fn panel_snapshot(
        &self,
        panel_id: PanelId,
        canvas_rect: Rect,
        workspaces: &[(WorkspaceId, String, Color32)],
    ) -> Option<PanelSnapshot> {
        self.board.panel(panel_id).and_then(|panel| {
            let terminal = panel.terminal();
            let canvas_position = Pos2::new(panel.layout.position[0], panel.layout.position[1]);
            let canvas_size = Vec2::new(panel.layout.size[0], panel.layout.size[1]);
            let screen_rect = Rect::from_min_size(
                self.canvas_to_screen(canvas_rect, canvas_position),
                self.canvas_size_to_screen(canvas_size),
            );

            // Cull off-screen panels — skip chrome, snapshot, and rendering.
            if !canvas_rect.intersects(screen_rect) {
                return None;
            }

            let workspace_accent = workspaces
                .iter()
                .find(|(workspace_id, _, _)| *workspace_id == panel.workspace_id)
                .map(|(_, _, color)| *color);

            let attention_badge = if self.template_config.features.attention_feed {
                self.board
                    .unresolved_attention_for_panel(panel_id)
                    .map(|item| (item.severity, item.summary.clone()))
            } else {
                None
            };

            Some(PanelSnapshot {
                screen_rect,
                canvas_position,
                canvas_size,
                current_workspace_id: panel.workspace_id,
                title: panel.title.clone(),
                display_title: panel.display_title().into_owned(),
                kind: panel.kind,
                history_size: terminal.map_or(0, horizon_core::Terminal::history_size),
                scrollback_limit: terminal.map_or(0, horizon_core::Terminal::scrollback_limit),
                workspace_accent,
                is_focused: self.board.focused == Some(panel_id),
                is_renaming: self.renaming_panel == Some(panel_id),
                attention_badge,
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
        workspaces: &[(WorkspaceId, String, Color32)],
    ) -> PanelUiOutcome {
        let mut outcome = PanelUiOutcome::default();

        egui::Area::new(Id::new(("panel", panel_id.0)))
            .fixed_pos(snapshot.canvas_position)
            .constrain(false)
            .interactable(false)
            .order(if snapshot.is_focused {
                Order::Foreground
            } else {
                Order::Middle
            })
            .show(ctx, |ui| {
                self.apply_canvas_layer_transform(ui, canvas_rect);
                let (panel_rect, _) = ui.allocate_exact_size(snapshot.canvas_size, Sense::hover());
                let rects = PanelFrame::new(panel_rect);
                let drag_response = ui.interact(
                    rects.titlebar,
                    ui.make_persistent_id(("panel_drag", panel_id.0)),
                    if snapshot.is_renaming {
                        Sense::hover()
                    } else {
                        Sense::click_and_drag()
                    },
                );
                let close_response = ui.interact(
                    rects.close.expand2(Vec2::splat(4.0)),
                    ui.make_persistent_id(("panel_close", panel_id.0)),
                    Sense::click(),
                );
                let resize_response = ui.interact(
                    rects.resize.expand2(Vec2::splat(6.0)),
                    ui.make_persistent_id(("panel_resize", panel_id.0)),
                    Sense::click_and_drag(),
                );

                Self::update_panel_interactions(
                    snapshot.is_renaming,
                    &drag_response,
                    &close_response,
                    &resize_response,
                    &mut outcome,
                );
                if !snapshot.is_renaming {
                    self.show_panel_context_menu(
                        &drag_response,
                        panel_id,
                        snapshot.current_workspace_id,
                        snapshot.kind,
                        workspaces,
                        &mut outcome,
                    );
                }

                paint_panel_chrome(
                    ui,
                    PanelChrome {
                        panel_id,
                        panel_rect: rects.panel,
                        titlebar_rect: rects.titlebar,
                        close_rect: rects.close,
                        resize_rect: rects.resize,
                        title: if snapshot.is_renaming {
                            None
                        } else {
                            Some(snapshot.display_title.as_str())
                        },
                        history_size: snapshot.history_size,
                        scrollback_limit: snapshot.scrollback_limit,
                        focused: snapshot.is_focused,
                        close_hovered: close_response.hovered(),
                        workspace_accent: snapshot.workspace_accent,
                        attention_badge: snapshot.attention_badge.as_ref(),
                    },
                );

                if snapshot.is_renaming {
                    outcome.rename_action = show_inline_rename_editor(
                        ui,
                        panel_title_content_rect(rects.titlebar, rects.close, snapshot.workspace_accent.is_some()),
                        &mut self.panel_rename_buffer,
                        egui::FontId::proportional(13.0),
                    );
                }

                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(rects.body)
                        .layout(Layout::top_down(Align::Min)),
                    |ui| {
                        let board = &mut self.board;
                        let terminal_grid_cache = &mut self.terminal_grid_cache;
                        if let Some(panel) = board.panel_mut(panel_id) {
                            let grid_cache = if panel.terminal().is_some() {
                                Some(terminal_grid_cache.entry(panel_id).or_default())
                            } else {
                                None
                            };
                            outcome.focus_requested |=
                                show_panel_body_contents(ui, panel, snapshot.is_focused, grid_cache);
                        }
                    },
                );
            });

        outcome
    }

    fn update_panel_interactions(
        is_renaming: bool,
        drag_response: &egui::Response,
        close_response: &egui::Response,
        resize_response: &egui::Response,
        outcome: &mut PanelUiOutcome,
    ) {
        if resize_response.drag_started() || resize_response.clicked() {
            outcome.focus_requested = true;
        }
        if !is_renaming && (drag_response.clicked() || drag_response.drag_started()) {
            outcome.focus_requested = true;
        }
        if !is_renaming && drag_response.dragged() {
            outcome.drag_delta = drag_response.drag_delta();
        }
        if resize_response.dragged() {
            outcome.resize_delta = resize_response.drag_delta();
        }
        if resize_response.drag_stopped() {
            outcome.commit_terminal_resize = true;
        }
        if close_response.clicked() {
            outcome.command = Some(PanelCommand::Close);
        }
        if !is_renaming && drag_response.double_clicked() {
            outcome.command = Some(PanelCommand::StartRename);
            outcome.focus_requested = true;
        }
    }

    fn show_panel_context_menu(
        &mut self,
        drag_response: &egui::Response,
        panel_id: PanelId,
        current_workspace_id: WorkspaceId,
        kind: PanelKind,
        workspaces: &[(WorkspaceId, String, Color32)],
        outcome: &mut PanelUiOutcome,
    ) {
        drag_response.context_menu(|ui| {
            ui.set_min_width(180.0);
            ui.label(egui::RichText::new("Move to Workspace").size(11.0).color(theme::FG_DIM));
            ui.separator();

            for (workspace_id, workspace_name, workspace_color) in workspaces {
                let is_current = current_workspace_id == *workspace_id;
                let label = if is_current {
                    format!("● {workspace_name}")
                } else {
                    format!("  {workspace_name}")
                };
                let text = egui::RichText::new(label)
                    .color(if is_current { *workspace_color } else { theme::FG_SOFT })
                    .size(12.0);
                if ui.add(egui::Button::new(text).frame(false)).clicked() {
                    outcome.workspace_assignment = Some(*workspace_id);
                    ui.close();
                }
            }

            ui.separator();
            // Compute rebind options lazily — only when the context menu is
            // actually open instead of every frame for every panel.
            let rebind_options = self.session_rebind_options(panel_id);
            if !rebind_options.is_empty() {
                ui.menu_button("Rebind Session", |ui| {
                    ui.set_min_width(220.0);
                    for (label, binding) in &rebind_options {
                        let button =
                            egui::Button::new(egui::RichText::new(label).size(12.0).color(theme::FG_SOFT)).frame(false);
                        if ui.add(button).clicked() {
                            self.pending_session_rebinds.push((panel_id, binding.clone()));
                            ui.close();
                        }
                    }
                });
                ui.separator();
            }
            if ui.button("New Workspace").clicked() {
                outcome.command = Some(PanelCommand::CreateWorkspace);
                ui.close();
            }
            if kind.is_agent() {
                ui.separator();
                if ui.button("Restart").clicked() {
                    self.panels_to_restart.push(panel_id);
                    ui.close();
                }
            }
        });
    }

    fn apply_panel_outcome(
        &mut self,
        ctx: &Context,
        panel_id: PanelId,
        snapshot: &PanelSnapshot,
        outcome: &PanelUiOutcome,
    ) -> bool {
        self.panel_screen_rects.insert(panel_id, snapshot.screen_rect);

        if matches!(outcome.command, Some(PanelCommand::StartRename)) {
            self.clear_workspace_rename();
            self.renaming_panel = Some(panel_id);
            self.panel_rename_buffer.clone_from(&snapshot.title);
        }

        match outcome.rename_action {
            RenameEditAction::Commit => {
                if self.renaming_panel == Some(panel_id) {
                    let name = self.panel_rename_buffer.trim().to_string();
                    if !name.is_empty() && self.board.rename_panel(panel_id, &name) {
                        self.mark_runtime_dirty();
                    }
                    self.clear_panel_rename();
                }
            }
            RenameEditAction::Cancel => {
                if self.renaming_panel == Some(panel_id) {
                    self.clear_panel_rename();
                }
            }
            RenameEditAction::None => {}
        }

        if !self.is_panning && outcome.drag_delta != Vec2::ZERO {
            let new_position = snapshot.canvas_position + outcome.drag_delta;
            let _ = self.board.move_panel(panel_id, [new_position.x, new_position.y]);
            self.mark_runtime_dirty();
        }
        if !self.is_panning && outcome.resize_delta != Vec2::ZERO {
            let new_size = clamp_panel_size(snapshot.canvas_size + outcome.resize_delta);
            let _ = self.board.resize_panel(panel_id, [new_size.x, new_size.y]);
            self.mark_runtime_dirty();
        }
        if outcome.commit_terminal_resize {
            let resized_panel_size = if outcome.resize_delta == Vec2::ZERO {
                snapshot.canvas_size
            } else {
                clamp_panel_size(snapshot.canvas_size + outcome.resize_delta)
            };
            let panel_rect = Rect::from_min_size(Pos2::ZERO, resized_panel_size);
            let body_size = PanelFrame::new(panel_rect).body.size();
            let viewport = viewport_for_available_space(ctx, body_size);
            if let Some(panel) = self.board.panel_mut(panel_id) {
                panel.resize_immediately(viewport.rows, viewport.cols, viewport.cell_width, viewport.cell_height);
            }
            ctx.request_repaint();
        }
        if outcome.focus_requested {
            self.board.focus(panel_id);
        }
        if matches!(outcome.command, Some(PanelCommand::CreateWorkspace)) {
            self.workspace_creates.push(panel_id);
        }
        if let Some(workspace_id) = outcome.workspace_assignment {
            self.workspace_assignments.push((panel_id, workspace_id));
        }

        matches!(outcome.command, Some(PanelCommand::Close))
    }
}
