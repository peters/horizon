mod fullscreen;
mod ownership;
mod production;
mod render;
mod runtime;

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use horizon_core::cloud_panel::{self, CHILD_SIZE, CLOUDS, CloudGroup, CloudGroups, PrototypeSnapshot};
use horizon_core::{Board, PanelId, PanelKind, PanelOptions, PanelResume, RuntimeState, WorkspaceId};

use super::HorizonApp;

type SetupResult = Result<(Vec<PathBuf>, Option<PrototypeSnapshot>, cloud_panel::CloudConfig), String>;

#[derive(Default)]
pub(super) struct CloudPrototype {
    production: production::Production,
    pub groups: CloudGroups,
    root: Option<PathBuf>,
    setup: Option<Receiver<SetupResult>>,
    initialized: bool,
    ready: bool,
    pub(super) error: Option<String>,
    last_save: Option<Instant>,
    renaming: Option<u32>,
    title_draft: String,
    profiles: Option<cloud_panel::CloudConfig>,
    provider_logo: Option<egui::TextureHandle>,
    pub fullscreen: Option<fullscreen::CloudFullscreen>,
    deployments: std::collections::HashMap<u32, runtime::DemoDeployment>,
}

impl CloudPrototype {
    pub(super) fn creation_open(&self) -> bool {
        self.production.creating || self.production.setup.open
    }
}

impl HorizonApp {
    pub(super) fn prepare_cloud_prototype(&mut self, ctx: &egui::Context) {
        if std::env::var_os("HORIZON_CLOUD_MOCK_DIR").is_none() {
            self.prepare_production_clouds(ctx);
            return;
        }
        self.start_cloud_setup(ctx);
        if self.pending_startup_runtime_state.is_some() || self.startup_receiver.is_some() {
            return;
        }
        let result = self.cloud_prototype.setup.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(result) = result {
            self.cloud_prototype.setup = None;
            match result {
                Ok((paths, snapshot, profiles)) => {
                    if !self.board.panels.is_empty() || self.active_session.as_ref().is_some_and(|s| s.persistent) {
                        self.cloud_prototype.error =
                            Some("Launch the prototype with an empty, ephemeral configuration.".into());
                        return;
                    }
                    let fresh = snapshot.is_none();
                    if let Some(mut snapshot) = snapshot {
                        snapshot.runtime.browser = self.template_config.browser.clone();
                        match Board::from_runtime_state_with_transcripts(
                            &snapshot.runtime,
                            self.transcript_root.as_deref(),
                        ) {
                            Ok(board) => {
                                self.board = board;
                                self.cloud_prototype.groups = snapshot.groups;
                                self.cloud_prototype.groups.restore_visibility(&mut self.board);
                                if let Some(view) = snapshot.runtime.canvas_view {
                                    self.canvas_view = view;
                                }
                            }
                            Err(e) => {
                                self.cloud_prototype.error = Some(e.to_string());
                                return;
                            }
                        }
                    } else {
                        let ws = self.board.create_workspace("Project desk");
                        if let Some(workspace) = self.board.workspace_mut(ws) {
                            workspace.layout = None;
                            workspace.position = [0.0, 0.0];
                        }
                        let workspace = self.board.workspace(ws).map(|w| w.local_id.clone()).unwrap_or_default();
                        for (index, ((issue, title), cwd)) in CLOUDS.into_iter().zip(paths).take(3).enumerate() {
                            let position = [[24.0, 100.0], [24.0, 888.0], [1450.0, 100.0]][index];
                            self.cloud_prototype.groups.0.push(CloudGroup::new(
                                issue,
                                title.into(),
                                workspace.clone(),
                                cwd,
                                position,
                            ));
                        }
                    }
                    if !self.cloud_prototype.groups.0.is_empty() {
                        self.cloud_prototype.groups.make_room(&mut self.board, 0);
                    }
                    self.cloud_prototype.ready = true;
                    self.initial_pan_done = true;
                    self.startup_workspace_organization_pending = false;
                    if fresh {
                        self.seed_cloud_scenarios(ctx, &profiles);
                    }
                    self.cloud_prototype.profiles = Some(profiles);
                    self.cloud_overview(ctx);
                }
                Err(e) => self.cloud_prototype.error = Some(e),
            }
        }
        self.advance_cloud_deployments(ctx);
        self.cloud_prototype
            .groups
            .0
            .retain(|g| self.board.workspace_id_by_local_id(&g.workspace).is_some());
        self.cloud_prototype.groups.adopt_intersecting(&self.board);
        self.cloud_prototype.groups.reconcile(&mut self.board);
        for group in &mut self.cloud_prototype.groups.0 {
            if let Some(id) = self.board.workspace_id_by_local_id(&group.workspace) {
                if let Some(workspace) = self.board.workspace_mut(id) {
                    workspace.layout = None;
                }
                self.board.retain_workspace_when_empty(id);
            }
        }
        if self.cloud_prototype.ready
            && self
                .cloud_prototype
                .last_save
                .is_none_or(|t| t.elapsed() > Duration::from_secs(1))
        {
            self.save_cloud_prototype();
        }
    }

    fn seed_cloud_scenarios(&mut self, ctx: &egui::Context, profiles: &cloud_panel::CloudConfig) {
        for group in &mut self.cloud_prototype.groups.0 {
            let scenario_profile = match group.issue {
                101 => "app",
                102 => "cuda",
                103 => "fly",
                _ => &profiles.default,
            };
            let name = if profiles.profiles.contains_key(scenario_profile) {
                scenario_profile
            } else {
                &profiles.default
            };
            let profile = &profiles.profiles[name];
            group.environment.profile = Some(name.to_string());
            group.environment.provider = Some(profile.provider.clone());
            group.environment.image.clone_from(&profile.image);
        }
        self.cloud_add_panel(ctx, 101, PanelKind::Browser, Some("https://www.daytona.io".into()));
        if let Some(panel) = self.board.panels.iter_mut().find(|p| p.kind == PanelKind::Browser) {
            panel.resize_layout([1060.0, 500.0]);
        }
        for group in &mut self.cloud_prototype.groups.0 {
            group.set_layout(&mut self.board, Some(horizon_core::WorkspaceLayout::Grid));
            self.cloud_prototype
                .deployments
                .insert(group.issue, runtime::DemoDeployment::scenario(group.issue));
        }
    }

    fn start_cloud_setup(&mut self, ctx: &egui::Context) {
        if !self.cloud_prototype.initialized {
            self.cloud_prototype.initialized = true;
            self.cloud_prototype.root = std::env::var_os("HORIZON_CLOUD_MOCK_DIR").map(PathBuf::from);
            if let Some(root) = self.cloud_prototype.root.clone() {
                let (tx, rx) = channel();
                self.cloud_prototype.setup = Some(rx);
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let result = cloud_panel::prepare_repository(&root)
                        .and_then(|paths| {
                            let yaml = std::fs::read_to_string(root.join("sample-project/.horizon/cloud.yml"))?;
                            let profiles = cloud_panel::CloudConfig::parse_design_fixture(&yaml)
                                .map_err(|e| horizon_core::Error::Config(e.to_string()))?;
                            cloud_panel::load(&root).map(|snapshot| (paths, snapshot, profiles))
                        })
                        .map_err(|e| e.to_string());
                    let _ = tx.send(result);
                    ctx.request_repaint();
                });
            }
        }
    }

    /// Whether `cloud_prototype.groups`, rather than the board's copy, holds this session's clouds.
    fn cloud_state_is_live(&self) -> bool {
        self.cloud_prototype.initialized
            && self.cloud_prototype.ready
            && (std::env::var_os("HORIZON_CLOUD_MOCK_DIR").is_some() || self.cloud_state_matches_session())
    }

    /// Resize a cloud member; a cloud that grows pushes neighbouring workspaces like a growing panel.
    pub(super) fn resize_cloud_member(
        &mut self,
        id: PanelId,
        size: [f32; 2],
        workspace_collision_ids: &[WorkspaceId],
    ) -> bool {
        if !self.cloud_prototype.groups.contains_panel(&self.board, id) {
            return false;
        }
        let live = self.cloud_state_is_live();
        let workspace = self.board.panel_workspace_id(id);
        let mut before = None;
        if live {
            std::mem::swap(&mut self.board.cloud_groups, &mut self.cloud_prototype.groups);
            before = workspace.and_then(|workspace| self.board.workspace_frame_rect(workspace));
            std::mem::swap(&mut self.board.cloud_groups, &mut self.cloud_prototype.groups);
        }
        self.cloud_prototype.groups.resize_panel(&mut self.board, id, size);
        if live && let Some(workspace) = workspace {
            self.board.cloud_groups.clone_from(&self.cloud_prototype.groups);
            self.board
                .resolve_workspace_frame_growth_in_scope(workspace, before, workspace_collision_ids);
        }
        true
    }

    /// Resize a panel outside every cloud; its growth pushes whole cloud frames like sibling panels.
    pub(super) fn resize_ordinary_panel(
        &mut self,
        id: PanelId,
        size: [f32; 2],
        workspace_collision_ids: &[WorkspaceId],
    ) {
        let live = self.cloud_state_is_live();
        if live {
            std::mem::swap(&mut self.board.cloud_groups, &mut self.cloud_prototype.groups);
        }
        let _ = self
            .board
            .resize_panel_with_workspace_scope(id, size, workspace_collision_ids);
        if live {
            self.cloud_prototype.groups.clone_from(&self.board.cloud_groups);
        }
    }

    pub(super) fn save_cloud_prototype(&mut self) {
        if !self.cloud_state_is_live() {
            return;
        }
        let mock = std::env::var_os("HORIZON_CLOUD_MOCK_DIR").is_some();
        self.board.cloud_groups = self.cloud_prototype.groups.clone();
        if !mock {
            self.mark_runtime_dirty();
            return;
        }
        let Some(root) = self.cloud_prototype.root.as_ref() else {
            return;
        };
        let view = self
            .cloud_prototype
            .fullscreen
            .as_ref()
            .map_or(self.canvas_view, |f| f.previous_view);
        let runtime = RuntimeState::from_board(&self.board, self.window_config.clone(), view);
        let snapshot = PrototypeSnapshot {
            version: 1,
            groups: self.cloud_prototype.groups.clone(),
            runtime,
        };
        if let Err(e) = cloud_panel::save(root, &snapshot) {
            self.cloud_prototype.error = Some(e.to_string());
        }
        self.cloud_prototype.last_save = Some(Instant::now());
    }

    pub(super) fn add_mock_cloud(&mut self, ctx: &egui::Context) {
        if std::env::var_os("HORIZON_CLOUD_MOCK_DIR").is_none() {
            self.cloud_prototype.production.creating = true;
            self.cloud_prototype.production.focus_title_on_open = true;
            return;
        }
        let Some(&(id, title)) = CLOUDS
            .iter()
            .find(|(id, _)| !self.cloud_prototype.groups.0.iter().any(|g| g.issue == *id))
        else {
            return;
        };
        let Some(root) = self.cloud_prototype.root.as_ref() else {
            return;
        };
        let cwd = root.join(format!("issue-{id}"));
        let ws = self.board.ensure_workspace();
        let Some(workspace) = self.board.workspace(ws) else {
            return;
        };
        let position = [
            24.0,
            self.cloud_prototype
                .groups
                .0
                .iter()
                .map(|g| g.bounds().1[1])
                .fold(80.0, f32::max)
                + 48.0,
        ];
        let mut group = CloudGroup::new(id, title.into(), workspace.local_id.clone(), cwd, position);
        if let Some(first) = self.cloud_prototype.groups.0.first() {
            group.environment.profile.clone_from(&first.environment.profile);
            group.environment.provider.clone_from(&first.environment.provider);
            group.environment.image.clone_from(&first.environment.image);
        }
        self.cloud_prototype
            .deployments
            .insert(id, runtime::DemoDeployment::default());
        self.cloud_prototype.groups.0.push(group);
        self.cloud_prototype.renaming = Some(id);
        self.cloud_prototype.title_draft = title.into();
        self.cloud_overview(ctx);
    }

    fn cloud_overview(&mut self, ctx: &egui::Context) {
        let mut min = [f32::MAX; 2];
        let mut max = [f32::MIN; 2];
        for group in &self.cloud_prototype.groups.0 {
            let (a, b) = group.overview_bounds();
            for axis in 0..2 {
                min[axis] = min[axis].min(a[axis]);
                max[axis] = max[axis].max(b[axis]);
            }
        }
        if !self.cloud_prototype.groups.0.is_empty() {
            self.cloud_fit(ctx, min, max);
        }
    }

    fn cloud_fit(&mut self, ctx: &egui::Context, min: [f32; 2], max: [f32; 2]) {
        let canvas = self.canvas_rect(ctx);
        self.pan_target = None;
        let mut view = CloudGroups::fitted_view(min, max, [canvas.width(), canvas.height()]);
        if self.cloud_prototype.fullscreen.is_none() {
            let controls = ctx
                .memory(|memory| memory.area_rect(egui::Id::new("cloud-controls")))
                .unwrap_or_else(|| {
                    egui::Rect::from_min_size(canvas.min + egui::vec2(24.0, 20.0), egui::vec2(370.0, 58.0))
                });
            let minimap = self.minimap_overlay_rect(ctx);
            let transform = crate::app::view::canvas_scene_transform(canvas, view);
            let overlaps = self.cloud_prototype.groups.0.iter().any(|group| {
                let (min, max) = group.overview_bounds();
                let bounds = transform * egui::Rect::from_min_max(min.into(), max.into());
                Some(controls)
                    .into_iter()
                    .chain(minimap)
                    .any(|overlay| bounds.intersects(overlay))
            });
            if overlaps {
                let top = controls.bottom() + 12.0;
                let bottom = minimap.map_or(canvas.bottom(), |rect| rect.top() - super::MINIMAP_MARGIN);
                view = CloudGroups::fitted_region(
                    min,
                    max,
                    [0.0, top - canvas.top()],
                    [canvas.width(), (bottom - top).max(1.0)],
                );
            }
        }
        if self.canvas_view != view {
            self.canvas_view = view;
            ctx.request_repaint();
        }
    }

    pub(super) fn cloud_attach_agent_child(&mut self, actor: horizon_core::PanelId, child: horizon_core::PanelId) {
        let Some(actor) = self.board.panel(actor) else {
            return;
        };
        let Some(index) = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .position(|g| g.panels.contains(&actor.local_id))
        else {
            return;
        };
        let position = self.cloud_prototype.groups.0[index].next_position(&self.board);
        if let Some(panel) = self.board.panel_mut(child) {
            panel.layout.position = position;
        }
        self.cloud_panel_created(index, child);
    }

    pub(super) fn cloud_panel_launch_group(
        &self,
        options: &mut PanelOptions,
        ws: horizon_core::WorkspaceId,
    ) -> Option<usize> {
        let index = self
            .cloud_prototype
            .groups
            .at_position(&self.board, ws, options.position?)?;
        if options.cwd.is_none() {
            options.cwd = Some(self.cloud_prototype.groups.0[index].cwd.clone());
        }
        if self
            .cloud_prototype
            .fullscreen
            .as_ref()
            .is_some_and(|view| view.id == self.cloud_prototype.groups.0[index].issue)
        {
            options.position = Some(self.cloud_prototype.groups.0[index].next_position(&self.board));
        }
        Some(index)
    }

    pub(super) fn cloud_panel_created(&mut self, index: usize, id: horizon_core::PanelId) {
        if self.cloud_prototype.groups.contains_panel(&self.board, id) {
            return;
        }
        self.cloud_prototype.groups.0[index].attach(&mut self.board, id);
        self.board.cloud_groups = self.cloud_prototype.groups.clone();
        self.cloud_prototype.groups.make_room(&mut self.board, index);
        if let Some(ws) = self.board.panel_workspace_id(id) {
            self.board.retain_workspace_when_empty(ws);
        }
    }

    fn cloud_add_panel(&mut self, ctx: &egui::Context, issue: u32, kind: PanelKind, endpoint: Option<String>) {
        let Some(index) = self.cloud_prototype.groups.0.iter().position(|g| g.issue == issue) else {
            return;
        };
        let group = &self.cloud_prototype.groups.0[index];
        let Some(ws) = self.board.workspace_id_by_local_id(&group.workspace) else {
            return;
        };
        let mut options = self.presets.iter().find(|p| p.kind == kind).map_or_else(
            || PanelOptions {
                kind,
                ..PanelOptions::default()
            },
            |p| p.to_panel_options(&self.template_config.browser),
        );
        options.cwd = Some(group.cwd.clone());
        options.resume = PanelResume::Fresh;
        options.position = Some(group.next_position(&self.board));
        options.size = Some(CHILD_SIZE);
        options.name = Some(kind.display_name().to_string());
        if let Some(endpoint) = endpoint {
            options.command = Some(endpoint);
        }
        if kind == PanelKind::Browser {
            options.browser_config = Some(self.template_config.browser.clone());
        }
        if let Some(workspace) = self.board.workspace_mut(ws) {
            workspace.layout = None;
        }
        options.transcript_root.clone_from(&self.transcript_root);
        if let Err(error) = self.prepare_cloud_remote_panel(index, &mut options) {
            self.cloud_prototype.error = Some(error.to_string());
            return;
        }
        match self.board.create_panel(options, ws) {
            Ok(id) => {
                self.cloud_panel_created(index, id);
                self.reveal_selected_panel(ctx, id);
                self.cloud_prototype.error = None;
                self.save_cloud_prototype();
            }
            Err(e) => self.cloud_prototype.error = Some(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::test_app;
    use crate::test_egui::DiscardTextures;

    #[test]
    fn fitted_clouds_clear_minimap_after_resize_and_preserve_wide_composition() {
        let (temp, mut app) = test_app();
        let workspace = app.board.create_workspace("fixture");
        let local = app.board.workspace(workspace).unwrap().local_id.clone();
        for (id, position, size) in [
            (101, [24.0, 100.0], [1088.0, 598.0]),
            (102, [238.0, 908.0], [1088.0, 598.0]),
            (103, [1667.0, 100.0], [1088.0, 1118.0]),
        ] {
            let mut group = CloudGroup::new(id, "Fixture".into(), local.clone(), temp.path().into(), position);
            group.size = size;
            app.cloud_prototype.groups.0.push(group);
        }
        let ctx = egui::Context::default();
        for (size, wide) in [
            (egui::vec2(1800.0, 980.0), true),
            (egui::vec2(1280.0, 720.0), false),
            (egui::vec2(1280.0, 600.0), false),
            (egui::vec2(1000.0, 600.0), false),
        ] {
            ctx.begin_pass(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                ..Default::default()
            });
            let controls = app.canvas_rect(&ctx).min + egui::vec2(24.0, 20.0);
            egui::Area::new(egui::Id::new("cloud-controls"))
                .fixed_pos(controls)
                .show(&ctx, |ui| {
                    ui.allocate_space(egui::vec2(370.0, 58.0));
                });
            app.cloud_overview(&ctx);
            let view = app.canvas_view;
            let canvas = app.canvas_rect(&ctx);
            let overlay = app.minimap_overlay_rect(&ctx).unwrap();
            let transform = crate::app::view::canvas_scene_transform(canvas, view);
            for group in &app.cloud_prototype.groups.0 {
                let (min, max) = group.overview_bounds();
                let bounds = transform * egui::Rect::from_min_max(min.into(), max.into());
                assert!(!bounds.intersects(overlay), "cloud must not hide behind minimap");
                assert!(canvas.contains_rect(bounds), "fitted cloud must remain on canvas");
                let controls = ctx
                    .memory(|memory| memory.area_rect(egui::Id::new("cloud-controls")))
                    .unwrap();
                assert!(
                    !bounds.intersects(controls),
                    "cloud header must clear overview controls"
                );
            }
            let persisted = RuntimeState::from_board(&app.board, app.window_config.clone(), view);
            let restored: RuntimeState = serde_json::from_str(&serde_json::to_string(&persisted).unwrap()).unwrap();
            assert_eq!(
                restored.canvas_view_or_default(),
                view,
                "small fitted overview survives persistence"
            );
            app.zoom_canvas_at(canvas, canvas.center(), view.zoom / 1.1);
            assert!(
                app.canvas_view.zoom < view.zoom,
                "zoom out must reduce magnification after fitting"
            );
            app.minimap_visible = false;
            app.cloud_overview(&ctx);
            if wide {
                assert_eq!(app.canvas_view, view, "wide approved composition needs no adjustment");
            } else {
                assert!(
                    app.canvas_view.zoom > view.zoom,
                    "narrow fit must reserve overlay space"
                );
            }
            app.minimap_visible = true;
            let _ = ctx.end_pass().discard_textures();
        }
    }

    #[test]
    fn cloud_launch_defaults_precede_workspace_cwd_and_binding_is_unique() {
        let (temp, mut app) = test_app();
        let (program, args): (String, Vec<String>) = if cfg!(windows) {
            ("cmd.exe".into(), vec!["/D".into(), "/C".into(), "exit".into()])
        } else {
            ("/bin/sh".into(), vec!["-c".into(), "true".into()])
        };
        let issue_dir = temp.path().join("issue");
        std::fs::create_dir(&issue_dir).unwrap();
        let ws = app.board.create_workspace_at("test", [0.0, 0.0]);
        let workspace = app.board.workspace_mut(ws).unwrap();
        workspace.cwd = Some(temp.path().to_path_buf());
        workspace.layout = None;
        let local = workspace.local_id.clone();
        app.cloud_prototype.groups = CloudGroups(vec![
            CloudGroup::new(1, "issue".into(), local.clone(), issue_dir.clone(), [0.0, 0.0]),
            CloudGroup::new(2, "overlap".into(), local, issue_dir.clone(), [0.0, 0.0]),
        ]);
        app.cloud_prototype.error = Some("Previous panel creation failed".into());
        let id = app
            .create_panel_with_options(
                PanelOptions {
                    kind: PanelKind::Shell,
                    command: Some(program.clone()),
                    args: args.clone(),
                    position: Some([20.0, 100.0]),
                    ..PanelOptions::default()
                },
                ws,
            )
            .unwrap();
        assert_eq!(app.board.panel(id).unwrap().launch_cwd.as_ref(), Some(&issue_dir));
        assert!(
            app.cloud_prototype.error.is_none(),
            "successful creation clears the previous error"
        );
        app.cloud_panel_created(1, id);
        assert_eq!(app.cloud_prototype.groups.0[0].panels.len(), 1);
        assert!(app.cloud_prototype.groups.0[1].panels.is_empty());
        let mut explicit = PanelOptions {
            cwd: Some(temp.path().into()),
            position: Some([20.0, 100.0]),
            ..PanelOptions::default()
        };
        app.cloud_panel_launch_group(&mut explicit, ws);
        assert_eq!(explicit.cwd.as_deref(), Some(temp.path()));
        app.cloud_prototype.error = Some("Another session failed to restore".into());
        let restored = app
            .board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Shell,
                    command: Some(program.clone()),
                    args: args.clone(),
                    ..PanelOptions::default()
                },
                ws,
            )
            .unwrap();
        app.cloud_panel_created(0, restored);
        assert_eq!(
            app.cloud_prototype.error.as_deref(),
            Some("Another session failed to restore")
        );
    }
    #[test]
    #[cfg_attr(windows, ignore = "agent panels launch through a POSIX login shell (#688)")]
    fn one_cloud_accepts_multiple_instances_of_each_agent() {
        let (temp, mut app) = test_app();
        let ws = app.board.create_workspace_at("test", [0.0, 0.0]);
        let local = app.board.workspace(ws).unwrap().local_id.clone();
        app.cloud_prototype.groups = CloudGroups(vec![CloudGroup::new(
            1,
            "Shared task".into(),
            local,
            temp.path().into(),
            [0.0, 0.0],
        )]);
        for kind in [PanelKind::Codex, PanelKind::Claude, PanelKind::Grok] {
            for _ in 0..2 {
                let id = app
                    .create_panel_with_options(
                        PanelOptions {
                            kind,
                            command: Some("/bin/true".into()),
                            resume: PanelResume::Fresh,
                            position: Some([20.0, 100.0]),
                            ..PanelOptions::default()
                        },
                        ws,
                    )
                    .unwrap();
                let panel = app.board.panel(id).unwrap();
                assert_eq!(panel.kind, kind);
                assert_eq!(panel.launch_cwd.as_deref(), Some(temp.path()));
                assert_eq!(panel.workspace_id, ws);
            }
            assert_eq!(app.board.panels.iter().filter(|p| p.kind == kind).count(), 2);
        }
        let members = &app.cloud_prototype.groups.0[0].panels;
        assert_eq!(members.len(), 6);
        assert_eq!(members.iter().collect::<std::collections::HashSet<_>>().len(), 6);
    }
}
