mod paint;
mod render;
mod toolbar;

use std::collections::HashMap;

use egui::{Color32, Context, Pos2, Rect, Vec2};
use horizon_core::{WorkspaceDockSide, WorkspaceId, WorkspaceLayout};

use crate::theme;

use super::util::{OverlayExclusion, workspace_label_width};
use super::{HorizonApp, RenameEditAction, WS_BG_PAD, WS_LABEL_HEIGHT, WS_TITLE_HEIGHT};

use self::render::render_workspace_visual;
use self::toolbar::workspace_layout_toolbar_rect;

pub(super) struct WorkspaceLayoutCapabilities {
    pub can_arrange: bool,
    pub can_detach: bool,
    /// The workspace holds nothing, and the board keeps another one, so a person may remove it.
    pub can_remove_empty: bool,
}

struct WorkspaceVisual {
    id: WorkspaceId,
    name: String,
    color: Color32,
    canvas_rect: Rect,
    screen_rect: Rect,
    label_canvas_rect: Rect,
    toolbar_canvas_rect: Rect,
    toolbar_screen_rect: Rect,
    is_active: bool,
    is_empty: bool,
    label_hidden: bool,
    panel_count: usize,
    layout: Option<WorkspaceLayout>,
    capabilities: WorkspaceLayoutCapabilities,
}

struct WorkspaceInteraction {
    activate_workspace: bool,
    drag_delta: Vec2,
    drag_stopped: bool,
    start_rename: bool,
    rename_action: RenameEditAction,
    action: Option<WorkspaceAction>,
}

enum WorkspaceAction {
    Focus,
    Fit,
    ClearLayout,
    ArrangeLayout(WorkspaceLayout),
    CloseAllPanels,
    Detach,
    AddPanel,
}

const WORKSPACE_LAYOUT_BUTTON_HEIGHT: f32 = 24.0;
const WORKSPACE_LAYOUT_BUTTON_SPACING: f32 = 4.0;
const WORKSPACE_LAYOUT_DEFAULT_BUTTON_WIDTH: f32 = 60.0;
const WORKSPACE_LAYOUT_TOOLBAR_MARGIN_X: i8 = 6;
const WORKSPACE_LAYOUT_TOOLBAR_MARGIN_Y: i8 = 5;
const WORKSPACE_LAYOUT_TOOLBAR_OFFSET_X: f32 = 10.0;
const WORKSPACE_DOCK_SNAP_DISTANCE: f32 = 72.0;

#[derive(Clone, Copy)]
struct WorkspaceDockTarget {
    dragged_workspace_id: WorkspaceId,
    workspace_id: WorkspaceId,
    side: WorkspaceDockSide,
}

impl HorizonApp {
    pub(super) fn workspace_layout_capabilities(&self, id: WorkspaceId) -> WorkspaceLayoutCapabilities {
        WorkspaceLayoutCapabilities {
            can_arrange: self.workspace_can_arrange_panels(id),
            can_detach: self.workspace_can_detach(id),
            can_remove_empty: self.board.can_remove_empty_workspace(id),
        }
    }

    pub(super) fn workspace_can_arrange_panels(&self, id: WorkspaceId) -> bool {
        self.board.workspace_accepts_panel_layout(id)
    }

    #[profiling::function]
    pub(super) fn render_workspace_backgrounds(
        &mut self,
        ctx: &Context,
        workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
        overlay_zones: &OverlayExclusion,
    ) {
        let show_layout_toolbar = true;
        self.render_workspace_backgrounds_in_rect(
            ctx,
            workspace_bounds,
            overlay_zones,
            self.canvas_rect(ctx),
            None,
            show_layout_toolbar,
        );
    }

    #[profiling::function]
    pub(super) fn render_detached_workspace_backgrounds(
        &mut self,
        ctx: &Context,
        workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
        canvas_rect: Rect,
        workspace_id: WorkspaceId,
    ) {
        self.render_workspace_backgrounds_in_rect(
            ctx,
            workspace_bounds,
            &OverlayExclusion::new(Vec::new()),
            canvas_rect,
            Some(workspace_id),
            false,
        );
    }

    #[profiling::function]
    fn render_workspace_backgrounds_in_rect(
        &mut self,
        ctx: &Context,
        workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
        overlay_zones: &OverlayExclusion,
        canvas_rect: Rect,
        visible_detached_workspace: Option<WorkspaceId>,
        show_layout_toolbar: bool,
    ) {
        let canvas_transform = super::view::canvas_scene_transform(canvas_rect, self.canvas_view);
        let canvas_clip_rect = canvas_transform.inverse() * canvas_rect;
        let visuals = self.workspace_visuals(canvas_rect, workspace_bounds, overlay_zones, visible_detached_workspace);
        let workspace_collision_ids = self.workspace_collision_scope(visible_detached_workspace);

        self.workspace_screen_rects.clear();
        let mut pending_workspace_moves = Vec::new();
        let mut focus_workspace = None;
        let mut start_rename_workspace = None;
        let mut rename_action = RenameEditAction::None;
        let mut clear_workspace_layout = None;
        let mut arrange_workspace = None;
        let mut close_workspace_panels = None;
        let mut focus_workspace_view = None;
        let mut fit_workspace_view = None;
        let mut dock_workspace = None;

        for workspace in &visuals {
            self.workspace_screen_rects.push((workspace.id, workspace.screen_rect));

            let is_renaming = self.renaming_workspace == Some(workspace.id);
            let interaction = if is_renaming {
                render_workspace_visual(
                    ctx,
                    workspace,
                    Some(&mut self.rename_buffer),
                    overlay_zones,
                    show_layout_toolbar,
                    canvas_transform,
                    canvas_clip_rect,
                )
            } else {
                render_workspace_visual(
                    ctx,
                    workspace,
                    None,
                    overlay_zones,
                    show_layout_toolbar,
                    canvas_transform,
                    canvas_clip_rect,
                )
            };

            if interaction.activate_workspace {
                focus_workspace = Some(workspace.id);
            }
            if interaction.drag_delta != Vec2::ZERO {
                pending_workspace_moves.push((workspace.id, interaction.drag_delta));
            }
            if interaction.drag_stopped {
                dock_workspace = workspace_dock_target(
                    workspace.id,
                    workspace.screen_rect.translate(interaction.drag_delta),
                    &visuals,
                );
            }
            if interaction.start_rename {
                start_rename_workspace = Some((workspace.id, workspace.name.clone()));
            }
            if interaction.rename_action != RenameEditAction::None {
                rename_action = interaction.rename_action;
            }
            match interaction.action {
                Some(WorkspaceAction::Focus) => {
                    focus_workspace_view = Some(workspace.id);
                }
                Some(WorkspaceAction::Fit) => {
                    fit_workspace_view = Some(workspace.id);
                }
                Some(WorkspaceAction::ClearLayout) => {
                    focus_workspace = Some(workspace.id);
                    clear_workspace_layout = Some(workspace.id);
                }
                Some(WorkspaceAction::ArrangeLayout(layout)) => {
                    focus_workspace = Some(workspace.id);
                    arrange_workspace = Some((workspace.id, layout));
                }
                Some(WorkspaceAction::Detach) => {
                    focus_workspace = Some(workspace.id);
                    self.detach_workspace(workspace.id);
                }
                Some(WorkspaceAction::AddPanel) => {
                    // The list creates its panel in the active workspace, so the
                    // press makes this one active before the person chooses.
                    focus_workspace = Some(workspace.id);
                    self.open_preset_palette();
                }
                Some(WorkspaceAction::CloseAllPanels) => {
                    focus_workspace = Some(workspace.id);
                    close_workspace_panels = Some(workspace.id);
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
        // Focus/fit for the workspace shown in a detached window must target
        // that window's canvas rect; the visible-canvas helpers reject
        // detached workspaces because they only pan the main canvas.
        if let Some(workspace_id) = focus_workspace_view {
            if visible_detached_workspace == Some(workspace_id) {
                let _ = self.focus_workspace_in_rect(workspace_id, canvas_rect);
            } else {
                let _ = self.focus_workspace_visible(ctx, workspace_id, false);
            }
        }
        if let Some(workspace_id) = fit_workspace_view {
            if visible_detached_workspace == Some(workspace_id) {
                let _ = self.fit_workspace_in_rect(workspace_id, canvas_rect);
            } else {
                let _ = self.fit_workspace_visible(ctx, workspace_id);
            }
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
        if let Some(workspace_id) = close_workspace_panels {
            self.close_workspace_panels(workspace_id);
        }

        if !self.canvas_pan_input_claimed {
            for (workspace_id, delta) in pending_workspace_moves {
                if dock_workspace.is_some_and(|target| target.dragged_workspace_id == workspace_id) {
                    continue;
                }
                let _ = self.board.translate_workspace_with_push_in_scope(
                    workspace_id,
                    [delta.x, delta.y],
                    &workspace_collision_ids,
                );
                self.mark_runtime_dirty();
            }
            if let Some(target) = dock_workspace
                && self.board.move_workspace_beside_in_scope(
                    target.dragged_workspace_id,
                    target.workspace_id,
                    target.side,
                    &workspace_collision_ids,
                )
            {
                self.mark_runtime_dirty();
            }
        }
    }

    #[profiling::function]
    fn workspace_visuals(
        &self,
        canvas_rect: Rect,
        workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
        overlay_zones: &OverlayExclusion,
        visible_detached_workspace: Option<WorkspaceId>,
    ) -> Vec<WorkspaceVisual> {
        self.board
            .workspaces
            .iter()
            .filter_map(|workspace| {
                if self.workspace_is_detached(workspace.id) && visible_detached_workspace != Some(workspace.id) {
                    return None;
                }

                let color = theme::workspace_accent(workspace.color_idx);
                let is_active = self.board.active_workspace == Some(workspace.id);
                let (workspace_canvas_rect, screen_rect, is_empty) =
                    if let Some((min, max)) = workspace_bounds.get(&workspace.id).copied() {
                        let top_left = Pos2::new(min[0] - WS_BG_PAD, min[1] - WS_BG_PAD - WS_TITLE_HEIGHT);
                        let bottom_right = Pos2::new(max[0] + WS_BG_PAD, max[1] + WS_BG_PAD);
                        let canvas_rect_local = Rect::from_min_max(top_left, bottom_right);
                        let screen_rect = Rect::from_min_size(
                            self.canvas_to_screen(canvas_rect, canvas_rect_local.min),
                            self.canvas_size_to_screen(canvas_rect_local.size()),
                        )
                        .intersect(canvas_rect);
                        (canvas_rect_local, screen_rect, false)
                    } else {
                        let canvas_rect_local = Rect::from_min_size(
                            Pos2::new(workspace.position[0], workspace.position[1]),
                            Self::empty_workspace_size(),
                        );
                        let screen_rect = Rect::from_min_size(
                            self.canvas_to_screen(canvas_rect, canvas_rect_local.min),
                            self.canvas_size_to_screen(canvas_rect_local.size()),
                        )
                        .intersect(canvas_rect);
                        (canvas_rect_local, screen_rect, true)
                    };

                // Cull off-screen workspaces to avoid painting backgrounds and
                // labels for workspaces the user cannot see.
                if !screen_rect.is_positive() || !canvas_rect.intersects(screen_rect) {
                    return None;
                }

                let label_canvas_rect = Rect::from_min_size(
                    workspace_canvas_rect.min + Vec2::new(14.0, 12.0),
                    Vec2::new(workspace_label_width(&workspace.name), WS_LABEL_HEIGHT),
                );
                let label_screen_rect = Rect::from_min_size(
                    self.canvas_to_screen(canvas_rect, label_canvas_rect.min),
                    self.canvas_size_to_screen(label_canvas_rect.size()),
                );
                let toolbar_canvas_rect = workspace_layout_toolbar_rect(label_canvas_rect);
                let toolbar_screen_rect = Rect::from_min_size(
                    self.canvas_to_screen(canvas_rect, toolbar_canvas_rect.min),
                    self.canvas_size_to_screen(toolbar_canvas_rect.size()),
                );
                Some(WorkspaceVisual {
                    id: workspace.id,
                    name: workspace.name.clone(),
                    color,
                    canvas_rect: workspace_canvas_rect,
                    screen_rect,
                    label_canvas_rect,
                    toolbar_canvas_rect,
                    toolbar_screen_rect,
                    is_active,
                    is_empty,
                    label_hidden: overlay_zones.intersects(label_screen_rect),
                    panel_count: workspace.panels.len(),
                    layout: workspace.layout,
                    capabilities: self.workspace_layout_capabilities(workspace.id),
                })
            })
            .collect()
    }
}

fn workspace_dock_target(
    dragged_workspace_id: WorkspaceId,
    dragged_screen_rect: Rect,
    visuals: &[WorkspaceVisual],
) -> Option<WorkspaceDockTarget> {
    visuals
        .iter()
        .filter(|workspace| workspace.id != dragged_workspace_id)
        .filter_map(|workspace| {
            workspace_dock_side(dragged_screen_rect, workspace.screen_rect).map(|side| {
                let delta = dragged_screen_rect.center() - workspace.screen_rect.center();
                (
                    WorkspaceDockTarget {
                        dragged_workspace_id,
                        workspace_id: workspace.id,
                        side,
                    },
                    delta.length_sq(),
                )
            })
        })
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(target, _)| target)
}

fn workspace_dock_side(dragged_rect: Rect, target_rect: Rect) -> Option<WorkspaceDockSide> {
    if !target_rect
        .expand(WORKSPACE_DOCK_SNAP_DISTANCE)
        .intersects(dragged_rect)
    {
        return None;
    }

    let delta = dragged_rect.center() - target_rect.center();
    if delta.x.abs() >= delta.y.abs() {
        Some(if delta.x <= 0.0 {
            WorkspaceDockSide::Left
        } else {
            WorkspaceDockSide::Right
        })
    } else {
        Some(if delta.y <= 0.0 {
            WorkspaceDockSide::Above
        } else {
            WorkspaceDockSide::Below
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{WorkspaceVisual, toolbar::should_show_workspace_layout_toolbar};
    use egui::{Color32, Context, Event, PointerButton, Pos2, Rect, Vec2};
    use horizon_core::{RuntimeState, StartupDecision, WorkspaceId};

    use crate::app::HorizonApp;
    use crate::app::test_support::{
        editor_workspace_state, raw_input, run_app_frame_with_input, test_app_with_startup,
    };
    const ADD_PANEL: &str = "+ Panel";

    /// The window of a whole application, with the clock that its frames need.
    struct AppWindow {
        ctx: Context,
        app: HorizonApp,
        time: f64,
        _temp: tempfile::TempDir,
        folder: tempfile::TempDir,
    }

    impl AppWindow {
        /// Two workspaces: `beta` holds a panel, and the empty `alpha` that the
        /// person makes after it is the active one and shows no workspace line.
        fn with_two_workspaces() -> Self {
            let folder = tempfile::tempdir().expect("a folder of the test");
            let mut beta = editor_workspace_state("beta", [520.0, 80.0]);
            beta.cwd = Some(folder.path().display().to_string());
            let (temp, ctx, app) = test_app_with_startup(StartupDecision::Ephemeral {
                runtime_state: Box::new(RuntimeState {
                    workspaces: vec![beta],
                    ..RuntimeState::default()
                }),
            });
            let mut window = Self {
                ctx,
                app,
                time: 1.0,
                _temp: temp,
                folder,
            };
            window.app.theme_applied = true;
            window.app.initial_pan_done = true;
            window.app.startup_workspace_organization_pending = false;
            window.app.root_viewport_stabilizer = None;
            window.ctx.enable_accesskit();
            // The startup frames move the canvas to the workspaces, and a press
            // follows the canvas that the person sees.
            window.frames_until("the startup view settles", |app| app.pan_target.is_none());
            // The active workspace is an empty one, so a press on the line of
            // `beta` proves that the choice follows the press and not the focus.
            let alpha = window.app.board.create_workspace("alpha");
            window.app.board.focus_workspace(alpha);
            window
        }

        fn workspace_id(&self, name: &str) -> WorkspaceId {
            self.app
                .board
                .workspaces
                .iter()
                .find(|workspace| workspace.name == name)
                .map(|workspace| workspace.id)
                .expect("the workspace of the state")
        }

        /// Every text that this frame painted, with the box of it.
        fn texts(output: &egui::FullOutput) -> Vec<(String, Rect)> {
            fn walk(shape: &egui::Shape, out: &mut Vec<(String, Rect)>) {
                match shape {
                    egui::Shape::Text(text) => out.push((
                        text.galley.text().to_owned(),
                        Rect::from_min_size(text.pos, text.galley.size()),
                    )),
                    egui::Shape::Vec(inner) => {
                        for shape in inner {
                            walk(shape, out);
                        }
                    }
                    _ => {}
                }
            }
            let mut out = Vec::new();
            for clipped in &output.shapes {
                walk(&clipped.shape, &mut out);
            }
            out
        }

        fn key(key: egui::Key) -> Event {
            Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }
        }

        fn frame(&mut self, events: Vec<Event>) -> egui::FullOutput {
            self.time += 0.016;
            let mut input = raw_input([1400.0, 900.0], None);
            input.time = Some(self.time);
            input.events = events;
            run_app_frame_with_input(&self.ctx, &mut self.app, input)
        }

        /// Where this frame draws the middle of the control of `label`.
        fn control(&mut self, label: &str) -> Option<Pos2> {
            let mut position = None;
            for _ in 0..2 {
                let output = self.frame(Vec::new());
                let Some(update) = output.platform_output.accesskit_update else {
                    continue;
                };
                position = update.nodes.iter().find_map(|(_, node)| {
                    (node.label().or_else(|| node.value()) == Some(label))
                        .then(|| node.bounds())
                        .flatten()
                });
            }
            #[allow(clippy::cast_possible_truncation)]
            position.map(|bounds| {
                // A control of the canvas answers its place on the canvas.
                let canvas = Pos2::new(
                    f64::midpoint(bounds.x0, bounds.x1) as f32,
                    f64::midpoint(bounds.y0, bounds.y1) as f32,
                );
                crate::app::view::canvas_scene_transform(self.app.canvas_rect(&self.ctx), self.app.canvas_view) * canvas
            })
        }

        fn press(&mut self, position: Pos2) {
            let button = |pressed: bool| Event::PointerButton {
                pos: position,
                button: PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::default(),
            };
            let _ = self.frame(vec![Event::PointerMoved(position)]);
            let _ = self.frame(vec![button(true)]);
            let _ = self.frame(vec![button(false)]);
        }

        fn frames_until(&mut self, what: &str, mut ready: impl FnMut(&HorizonApp) -> bool) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            loop {
                let _ = self.frame(Vec::new());
                if ready(&self.app) {
                    return;
                }
                assert!(std::time::Instant::now() < deadline, "{what}");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }

    #[test]
    fn the_panel_control_of_a_workspace_opens_the_presets_and_adds_one_there() {
        let mut window = AppWindow::with_two_workspaces();
        let beta = window.workspace_id("beta");
        assert_ne!(
            window.app.board.active_workspace,
            Some(beta),
            "the press adds a panel to a workspace that is not the active one"
        );
        let panels_before = window.app.board.panels.len();

        let position = window
            .control(ADD_PANEL)
            .expect("the workspace line of a workspace holds the control that adds a panel");
        window.press(position);

        window.frames_until("the press opens the list of the presets", |app| {
            app.command_palette.is_some()
        });
        assert_eq!(
            window.app.board.panels.len(),
            panels_before,
            "opening the list adds no panel by itself"
        );

        // The list holds the presets alone, in the order of the settings, so
        // Enter chooses the first one.
        let _ = window.frame(vec![AppWindow::key(egui::Key::Enter)]);

        window.frames_until("the preset that the person chose opens", |app| {
            app.board.panels.len() > panels_before
        });
        assert!(
            window
                .app
                .board
                .panels
                .iter()
                .any(|panel| { panel.workspace_id == beta && panel.kind == horizon_core::PanelKind::Shell }),
            "the preset that the person chose opened in the workspace of the press: {:?}",
            window
                .app
                .board
                .panels
                .iter()
                .map(|panel| (panel.kind, panel.workspace_id))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            window.app.board.panels.len(),
            panels_before + 1,
            "one panel joined the board"
        );
    }

    /// The settings list of a first start, with the program of each agent
    /// preset in the folder of the test.
    fn presets_of(folder: &std::path::Path) -> Vec<horizon_core::PresetConfig> {
        let preset = |name: &str, kind: horizon_core::PanelKind, program: Option<&str>| horizon_core::PresetConfig {
            name: name.to_owned(),
            alias: None,
            kind,
            command: program.map(|program| folder.join(program).display().to_string()),
            args: Vec::new(),
            resume: horizon_core::PanelResume::Fresh,
            ssh_connection: None,
        };
        vec![
            preset("Shell", horizon_core::PanelKind::Shell, None),
            preset("Codex", horizon_core::PanelKind::Codex, Some("codex")),
            preset("Claude Code", horizon_core::PanelKind::Claude, Some("claude")),
        ]
    }

    #[test]
    fn a_preset_without_its_program_stands_last_is_marked_and_still_starts() {
        let mut window = AppWindow::with_two_workspaces();
        let folder = window.folder.path().to_path_buf();
        horizon_core::stand_in_program_in(&folder, "claude", "exit 0\n").expect("the program of a test");
        window.app.presets = presets_of(&folder);
        let beta = window.workspace_id("beta");
        let panels_before = window.app.board.panels.len();

        let position = window
            .control(ADD_PANEL)
            .expect("the workspace line of a workspace holds the control that adds a panel");
        window.press(position);

        // The search of the programs runs on the worker, so the list is unmarked
        // until its answer is here.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut rows;
        loop {
            let output = window.frame(Vec::new());
            rows = AppWindow::texts(&output);
            if rows.iter().any(|(text, _)| text == "not installed") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the list marks the preset whose program this computer lacks: {rows:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let row = |label: &str| {
            rows.iter()
                .find(|(text, _)| text == label)
                .map_or_else(|| panic!("the list draws the row {label}: {rows:?}"), |(_, rect)| *rect)
        };
        let (shell, claude, codex) = (row("Shell"), row("Claude Code"), row("Codex"));
        assert!(
            shell.top() < claude.top() && claude.top() < codex.top(),
            "a preset without its program stands last: {rows:?}"
        );
        let marks: Vec<Rect> = rows
            .iter()
            .filter(|(text, _)| text == "not installed")
            .map(|(_, rect)| *rect)
            .collect();
        assert_eq!(marks.len(), 1, "one preset without its program: {rows:?}");
        assert!(
            (marks[0].center().y - codex.center().y).abs() < 1.0 && marks[0].left() > codex.right(),
            "the words stand at the right of the row of Codex: {marks:?} {codex:?}"
        );

        // The mark informs: the preset still starts, and it starts where the
        // person pressed.
        let _ = window.frame(vec![AppWindow::key(egui::Key::ArrowDown)]);
        let _ = window.frame(vec![AppWindow::key(egui::Key::ArrowDown)]);
        let _ = window.frame(vec![AppWindow::key(egui::Key::Enter)]);
        window.frames_until("the preset that the person chose opens", |app| {
            app.board.panels.len() > panels_before
        });
        assert!(
            window
                .app
                .board
                .panels
                .iter()
                .any(|panel| { panel.workspace_id == beta && panel.kind == horizon_core::PanelKind::Codex }),
            "the marked preset opened in the workspace of the press: {:?}",
            window
                .app
                .board
                .panels
                .iter()
                .map(|panel| (panel.kind, panel.workspace_id))
                .collect::<Vec<_>>()
        );
    }

    fn workspace_visual(panel_count: usize) -> WorkspaceVisual {
        let rect = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(120.0, 64.0));
        WorkspaceVisual {
            id: WorkspaceId(1),
            name: "Alpha".to_string(),
            color: Color32::WHITE,
            canvas_rect: rect,
            screen_rect: rect,
            label_canvas_rect: rect,
            toolbar_canvas_rect: rect,
            toolbar_screen_rect: rect,
            is_active: false,
            is_empty: panel_count == 0,
            label_hidden: false,
            panel_count,
            layout: None,
            capabilities: super::WorkspaceLayoutCapabilities {
                can_arrange: true,
                can_detach: true,
                can_remove_empty: false,
            },
        }
    }

    #[test]
    fn layout_toolbar_stays_hidden_for_empty_workspaces() {
        assert!(!should_show_workspace_layout_toolbar(&workspace_visual(0)));
    }

    #[test]
    fn layout_toolbar_stays_visible_for_single_panel_workspaces() {
        assert!(should_show_workspace_layout_toolbar(&workspace_visual(1)));
    }

    #[test]
    fn layout_toolbar_stays_for_a_workspace_of_only_hidden_panels() {
        // Hidden panels give a workspace no bounds, so it looks empty.
        let mut hidden_only = workspace_visual(2);
        hidden_only.is_empty = true;
        assert!(should_show_workspace_layout_toolbar(&hidden_only));
    }
}
