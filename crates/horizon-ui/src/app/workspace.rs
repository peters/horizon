use egui::{
    Button, Color32, Context, CornerRadius, CursorIcon, Id, Margin, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2,
};
use horizon_core::{WorkspaceId, WorkspaceLayout};

use crate::theme;

use super::util::workspace_label_width;
use super::{HorizonApp, RenameEditAction, WS_BG_PAD, WS_EMPTY_SIZE, WS_LABEL_HEIGHT, WS_TITLE_HEIGHT};

struct WorkspaceVisual {
    id: WorkspaceId,
    name: String,
    color: Color32,
    screen_rect: Rect,
    label_rect: Rect,
    is_active: bool,
    is_empty: bool,
    panel_count: usize,
    layout: Option<WorkspaceLayout>,
}

struct WorkspaceInteraction {
    activate_workspace: bool,
    drag_delta: Vec2,
    start_rename: bool,
    rename_action: RenameEditAction,
    show_layout_toolbar: bool,
    layout_action: Option<WorkspaceLayoutAction>,
}

#[derive(Clone, Copy)]
enum WorkspaceLayoutAction {
    Clear,
    Arrange(WorkspaceLayout),
}

const WORKSPACE_LAYOUT_BUTTON_HEIGHT: f32 = 24.0;
const WORKSPACE_LAYOUT_BUTTON_SPACING: f32 = 4.0;
const WORKSPACE_LAYOUT_DEFAULT_BUTTON_WIDTH: f32 = 60.0;
const WORKSPACE_LAYOUT_TOOLBAR_MARGIN_X: i8 = 6;
const WORKSPACE_LAYOUT_TOOLBAR_MARGIN_Y: i8 = 5;
const WORKSPACE_LAYOUT_TOOLBAR_OFFSET_X: f32 = 10.0;

impl HorizonApp {
    pub(super) fn render_workspace_backgrounds(&mut self, ctx: &Context) {
        let canvas_rect = Self::canvas_rect(ctx, self.sidebar_visible);
        let visuals = self.workspace_visuals(canvas_rect);

        self.workspace_screen_rects.clear();
        let mut pending_workspace_moves = Vec::new();
        let mut focus_workspace = None;
        let mut start_rename_workspace = None;
        let mut rename_action = RenameEditAction::None;
        let mut clear_workspace_layout = None;
        let mut arrange_workspace = None;

        for workspace in &visuals {
            self.workspace_screen_rects.push((workspace.id, workspace.screen_rect));

            let is_renaming = self.renaming_workspace == Some(workspace.id);
            let interaction = if is_renaming {
                render_workspace_visual(ctx, workspace, Some(&mut self.rename_buffer))
            } else {
                render_workspace_visual(ctx, workspace, None)
            };

            if interaction.activate_workspace {
                focus_workspace = Some(workspace.id);
            }
            if interaction.drag_delta != Vec2::ZERO {
                pending_workspace_moves.push((workspace.id, interaction.drag_delta));
            }
            if interaction.start_rename {
                start_rename_workspace = Some((workspace.id, workspace.name.clone()));
            }
            if interaction.rename_action != RenameEditAction::None {
                rename_action = interaction.rename_action;
            }
            match interaction.layout_action {
                Some(WorkspaceLayoutAction::Clear) => {
                    focus_workspace = Some(workspace.id);
                    clear_workspace_layout = Some(workspace.id);
                }
                Some(WorkspaceLayoutAction::Arrange(layout)) => {
                    focus_workspace = Some(workspace.id);
                    arrange_workspace = Some((workspace.id, layout));
                }
                None => {}
            }
        }

        if let Some((workspace_id, current_name)) = start_rename_workspace {
            self.clear_panel_rename();
            self.renaming_workspace = Some(workspace_id);
            self.rename_buffer = current_name;
        }

        match rename_action {
            RenameEditAction::Commit => {
                if let Some(workspace_id) = self.renaming_workspace {
                    let name = self.rename_buffer.trim().to_string();
                    if !name.is_empty() && self.board.rename_workspace(workspace_id, &name) {
                        self.mark_runtime_dirty();
                    }
                    self.clear_workspace_rename();
                }
            }
            RenameEditAction::Cancel => self.clear_workspace_rename(),
            RenameEditAction::None => {}
        }

        if let Some(workspace_id) = focus_workspace {
            self.board.focus_workspace(workspace_id);
        }
        if let Some(workspace_id) = clear_workspace_layout
            && self.board.clear_workspace_layout(workspace_id)
        {
            self.mark_runtime_dirty();
        }
        if let Some((workspace_id, layout)) = arrange_workspace {
            self.board.arrange_workspace(workspace_id, layout);
            self.mark_runtime_dirty();
        }

        if !self.is_panning {
            for (workspace_id, delta) in pending_workspace_moves {
                let _ = self
                    .board
                    .translate_workspace_with_push(workspace_id, [delta.x, delta.y]);
                self.mark_runtime_dirty();
            }
        }
    }

    fn workspace_visuals(&self, canvas_rect: Rect) -> Vec<WorkspaceVisual> {
        self.board
            .workspaces
            .iter()
            .map(|workspace| {
                let (r, g, b) = workspace.accent();
                let color = Color32::from_rgb(r, g, b);
                let is_active = self.board.active_workspace == Some(workspace.id);
                let (screen_rect, is_empty) = if let Some((min, max)) = self.board.workspace_bounds(workspace.id) {
                    let top_left = Pos2::new(min[0] - WS_BG_PAD, min[1] - WS_BG_PAD - WS_TITLE_HEIGHT);
                    let bottom_right = Pos2::new(max[0] + WS_BG_PAD, max[1] + WS_BG_PAD);
                    let screen_min = self.canvas_to_screen(canvas_rect, top_left);
                    let screen_max = self.canvas_to_screen(canvas_rect, bottom_right);
                    let clamped_min_y = screen_min.y.max(canvas_rect.min.y);
                    let clamped_max_y = screen_max.y.max(clamped_min_y);
                    (
                        Rect::from_min_max(
                            Pos2::new(screen_min.x, clamped_min_y),
                            Pos2::new(screen_max.x, clamped_max_y),
                        ),
                        false,
                    )
                } else {
                    let screen_min =
                        self.canvas_to_screen(canvas_rect, Pos2::new(workspace.position[0], workspace.position[1]));
                    (
                        Rect::from_min_size(
                            Pos2::new(screen_min.x, screen_min.y.max(canvas_rect.min.y)),
                            Vec2::new(WS_EMPTY_SIZE[0], WS_EMPTY_SIZE[1]),
                        ),
                        true,
                    )
                };

                WorkspaceVisual {
                    id: workspace.id,
                    name: workspace.name.clone(),
                    color,
                    screen_rect,
                    label_rect: Rect::from_min_size(
                        screen_rect.min + Vec2::new(14.0, 12.0),
                        Vec2::new(workspace_label_width(&workspace.name), WS_LABEL_HEIGHT),
                    ),
                    is_active,
                    is_empty,
                    panel_count: workspace.panels.len(),
                    layout: workspace.layout,
                }
            })
            .collect()
    }
}

fn render_workspace_visual(
    ctx: &Context,
    workspace: &WorkspaceVisual,
    rename_buffer: Option<&mut String>,
) -> WorkspaceInteraction {
    let is_renaming = rename_buffer.is_some();

    egui::Area::new(Id::new(("workspace_bg", workspace.id.0)))
        .fixed_pos(workspace.screen_rect.min)
        .constrain(false)
        .order(egui::Order::Background)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(workspace.screen_rect.size(), Sense::hover());
            paint_workspace_frame(ui, rect, workspace.color, workspace.is_active);

            if workspace.is_empty {
                paint_empty_workspace_hint(ui, rect, workspace.label_rect, workspace.color);
            }
        });

    let mut interaction = egui::Area::new(Id::new(("workspace_label", workspace.id.0)))
        .fixed_pos(workspace.label_rect.min)
        .constrain(false)
        .order(egui::Order::Tooltip)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(workspace.label_rect.size(), Sense::hover());
            let label_response = ui.interact(
                rect,
                ui.make_persistent_id(("workspace_drag", workspace.id.0)),
                Sense::click_and_drag(),
            );

            if let Some(buffer) = rename_buffer {
                paint_workspace_label_bg(ui, rect, workspace.color, true, false, false);
                WorkspaceInteraction {
                    activate_workspace: false,
                    drag_delta: Vec2::ZERO,
                    start_rename: false,
                    rename_action: super::panels::show_inline_rename_editor(
                        ui,
                        Rect::from_min_max(
                            Pos2::new(rect.min.x + 12.0, rect.min.y + 2.0),
                            Pos2::new(rect.max.x - 8.0, rect.max.y - 2.0),
                        ),
                        buffer,
                        egui::FontId::proportional(12.5),
                    ),
                    show_layout_toolbar: false,
                    layout_action: None,
                }
            } else {
                if label_response.hovered() || label_response.dragged() {
                    ui.ctx().set_cursor_icon(if label_response.dragged() {
                        CursorIcon::Grabbing
                    } else {
                        CursorIcon::Grab
                    });
                }

                paint_workspace_label(
                    ui,
                    rect,
                    &workspace.name,
                    workspace.color,
                    workspace.is_active,
                    label_response.hovered(),
                    label_response.dragged(),
                );

                WorkspaceInteraction {
                    activate_workspace: label_response.clicked() || label_response.drag_started(),
                    drag_delta: if label_response.dragged() {
                        ctx.input(|input| input.pointer.delta())
                    } else {
                        Vec2::ZERO
                    },
                    start_rename: label_response.double_clicked(),
                    rename_action: RenameEditAction::None,
                    show_layout_toolbar: label_response.hovered(),
                    layout_action: None,
                }
            }
        })
        .inner;

    if !is_renaming && should_show_workspace_layout_toolbar(ctx, workspace, interaction.show_layout_toolbar) {
        interaction.layout_action = render_workspace_layout_toolbar(ctx, workspace);
    }

    interaction
}

fn paint_workspace_frame(ui: &mut egui::Ui, rect: Rect, color: Color32, is_active: bool) {
    let painter = ui.painter_at(rect);
    let corner_radius = CornerRadius::same(20);
    let border_alpha = if is_active { 110 } else { 55 };
    let fill_alpha = if is_active { 24 } else { 14 };
    let frame_fill = theme::alpha(theme::blend(theme::PANEL_BG, color, 0.12), fill_alpha);

    painter.rect_filled(rect, corner_radius, frame_fill);
    painter.rect_stroke(
        rect,
        corner_radius,
        Stroke::new(1.0, theme::alpha(color, border_alpha)),
        StrokeKind::Outside,
    );
}

fn paint_workspace_label_bg(
    ui: &mut egui::Ui,
    rect: Rect,
    color: Color32,
    is_active: bool,
    hovered: bool,
    dragging: bool,
) {
    let painter = ui.painter();
    let tint = if dragging {
        0.22
    } else if hovered {
        0.18
    } else if is_active {
        0.14
    } else {
        0.08
    };
    let fill = theme::blend(theme::PANEL_BG_ALT, color, tint);
    let border_alpha = if is_active || hovered { 160 } else { 90 };

    painter.rect_filled(rect, CornerRadius::same(10), fill);
    painter.rect_stroke(
        rect,
        CornerRadius::same(10),
        Stroke::new(1.0, theme::alpha(color, border_alpha)),
        StrokeKind::Outside,
    );
}

fn paint_workspace_label(
    ui: &mut egui::Ui,
    rect: Rect,
    name: &str,
    color: Color32,
    is_active: bool,
    hovered: bool,
    dragging: bool,
) {
    paint_workspace_label_bg(ui, rect, color, is_active, hovered, dragging);

    let painter = ui.painter();
    let grip_center = Pos2::new(rect.max.x - 14.0, rect.center().y);

    painter.circle_filled(
        Pos2::new(rect.min.x + 14.0, rect.center().y),
        4.0,
        theme::alpha(color, if is_active { 220 } else { 150 }),
    );

    painter.text(
        Pos2::new(rect.min.x + 26.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(12.5),
        if is_active { theme::FG } else { theme::FG_SOFT },
    );

    paint_workspace_grip(painter, grip_center, dragging || hovered);
}

fn paint_workspace_grip(painter: &egui::Painter, center: Pos2, highlighted: bool) {
    let color = if highlighted {
        theme::alpha(theme::FG_SOFT, 180)
    } else {
        theme::alpha(theme::FG_DIM, 140)
    };
    let x_offsets = [-3.0, 3.0];
    let y_offsets = [-4.0, 0.0, 4.0];

    for x_offset in x_offsets {
        for y_offset in y_offsets {
            painter.circle_filled(Pos2::new(center.x + x_offset, center.y + y_offset), 1.2, color);
        }
    }
}

fn should_show_workspace_layout_toolbar(ctx: &Context, workspace: &WorkspaceVisual, label_hovered: bool) -> bool {
    if workspace.panel_count == 0 {
        return false;
    }

    if workspace.is_active || label_hovered {
        return true;
    }

    ctx.input(|input| input.pointer.hover_pos())
        .is_some_and(|pointer| workspace_layout_toolbar_rect(workspace).contains(pointer))
}

fn render_workspace_layout_toolbar(ctx: &Context, workspace: &WorkspaceVisual) -> Option<WorkspaceLayoutAction> {
    let toolbar_rect = workspace_layout_toolbar_rect(workspace);
    let mut action = None;

    egui::Area::new(Id::new(("workspace_layout_toolbar", workspace.id.0)))
        .fixed_pos(toolbar_rect.min)
        .constrain(false)
        .order(egui::Order::Tooltip)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme::alpha(
                    theme::blend(theme::PANEL_BG_ALT, workspace.color, 0.08),
                    228,
                ))
                .stroke(Stroke::new(1.0, theme::alpha(workspace.color, 112)))
                .corner_radius(10.0)
                .inner_margin(Margin::symmetric(
                    WORKSPACE_LAYOUT_TOOLBAR_MARGIN_X,
                    WORKSPACE_LAYOUT_TOOLBAR_MARGIN_Y,
                ))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = WORKSPACE_LAYOUT_BUTTON_SPACING;
                        let is_default = workspace.layout.is_none();
                        let response = ui
                            .add(
                                Button::new(egui::RichText::new("Default").size(10.5).color(if is_default {
                                    theme::FG
                                } else {
                                    theme::FG_SOFT
                                }))
                                .min_size(Vec2::new(
                                    WORKSPACE_LAYOUT_DEFAULT_BUTTON_WIDTH,
                                    WORKSPACE_LAYOUT_BUTTON_HEIGHT,
                                ))
                                .fill(if is_default {
                                    theme::alpha(theme::blend(theme::PANEL_BG_ALT, workspace.color, 0.22), 236)
                                } else {
                                    theme::alpha(theme::blend(theme::PANEL_BG_ALT, workspace.color, 0.05), 220)
                                })
                                .stroke(Stroke::new(
                                    1.0,
                                    if is_default {
                                        theme::alpha(workspace.color, 224)
                                    } else {
                                        theme::alpha(theme::blend(theme::BORDER_SUBTLE, workspace.color, 0.24), 216)
                                    },
                                ))
                                .corner_radius(8),
                            )
                            .on_hover_text("Manual placement");
                        if response.clicked() {
                            action = Some(WorkspaceLayoutAction::Clear);
                        }

                        for layout in WorkspaceLayout::ALL {
                            let is_selected = workspace.layout == Some(layout);
                            let response = ui
                                .add(
                                    Button::new(
                                        egui::RichText::new(workspace_layout_label(layout))
                                            .size(10.5)
                                            .color(if is_selected { theme::FG } else { theme::FG_SOFT }),
                                    )
                                    .min_size(Vec2::new(
                                        workspace_layout_button_width(layout),
                                        WORKSPACE_LAYOUT_BUTTON_HEIGHT,
                                    ))
                                    .fill(if is_selected {
                                        theme::alpha(theme::blend(theme::PANEL_BG_ALT, workspace.color, 0.22), 236)
                                    } else {
                                        theme::alpha(theme::blend(theme::PANEL_BG_ALT, workspace.color, 0.05), 220)
                                    })
                                    .stroke(Stroke::new(
                                        1.0,
                                        if is_selected {
                                            theme::alpha(workspace.color, 224)
                                        } else {
                                            theme::alpha(theme::blend(theme::BORDER_SUBTLE, workspace.color, 0.24), 216)
                                        },
                                    ))
                                    .corner_radius(8),
                                )
                                .on_hover_text(layout.label());
                            if response.clicked() {
                                action = Some(WorkspaceLayoutAction::Arrange(layout));
                            }
                        }
                    });
                });
        });

    action
}

fn workspace_layout_label(layout: WorkspaceLayout) -> &'static str {
    match layout {
        WorkspaceLayout::Rows => "Rows",
        WorkspaceLayout::Columns => "Cols",
        WorkspaceLayout::Grid => "Grid",
        WorkspaceLayout::Stack => "Stack",
        WorkspaceLayout::Cascade => "Cascade",
    }
}

fn workspace_layout_button_width(layout: WorkspaceLayout) -> f32 {
    match layout {
        WorkspaceLayout::Rows | WorkspaceLayout::Columns | WorkspaceLayout::Grid => 44.0,
        WorkspaceLayout::Stack => 52.0,
        WorkspaceLayout::Cascade => 68.0,
    }
}

fn workspace_layout_toolbar_rect(workspace: &WorkspaceVisual) -> Rect {
    Rect::from_min_size(
        Pos2::new(
            workspace.label_rect.max.x + WORKSPACE_LAYOUT_TOOLBAR_OFFSET_X,
            workspace.label_rect.min.y,
        ),
        Vec2::new(
            WORKSPACE_LAYOUT_DEFAULT_BUTTON_WIDTH
                + workspace_layout_preset_row_width()
                + 5.0 * WORKSPACE_LAYOUT_BUTTON_SPACING
                + 2.0 * f32::from(WORKSPACE_LAYOUT_TOOLBAR_MARGIN_X),
            WORKSPACE_LAYOUT_BUTTON_HEIGHT + 2.0 * f32::from(WORKSPACE_LAYOUT_TOOLBAR_MARGIN_Y),
        ),
    )
}

fn workspace_layout_preset_row_width() -> f32 {
    workspace_layout_button_width(WorkspaceLayout::Rows)
        + workspace_layout_button_width(WorkspaceLayout::Columns)
        + workspace_layout_button_width(WorkspaceLayout::Grid)
        + workspace_layout_button_width(WorkspaceLayout::Stack)
        + workspace_layout_button_width(WorkspaceLayout::Cascade)
}

fn paint_empty_workspace_hint(ui: &mut egui::Ui, rect: Rect, label_rect: Rect, color: Color32) {
    let painter = ui.painter();
    let copy_pos = Pos2::new(rect.min.x + 18.0, label_rect.max.y + 22.0);

    painter.text(
        copy_pos,
        egui::Align2::LEFT_TOP,
        "Drag this workspace anywhere on the board.",
        egui::FontId::proportional(12.0),
        theme::alpha(theme::FG_SOFT, 210),
    );
    painter.text(
        copy_pos + Vec2::new(0.0, 20.0),
        egui::Align2::LEFT_TOP,
        "New terminals will land inside this frame.",
        egui::FontId::proportional(10.5),
        theme::alpha(theme::blend(theme::FG_DIM, color, 0.18), 196),
    );
}
