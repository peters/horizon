use crate::app::HorizonApp;
use crate::dir_picker::{DirPicker, DirPickerPurpose};
use horizon_core::{
    MachineKeys, PanelId, PanelOptions, PanelResume, PanelTranscript, PlaceWorkspace, PresetConfig, WorkspaceId,
};

use super::support::preset_panel_options;
use super::{add_panel_position, inherit_workspace_cwd, workspace_cwd};

/// Whether a new panel of a preset asks where it runs before it opens: only on
/// this computer, and only when the directory of the workspace is not there. A
/// terminal of a workspace on a machine starts where the machine puts it, so the
/// person chooses no local directory for it.
fn asks_where_to_run(runs_here: bool, has_directory: bool, needs_directory: bool) -> bool {
    runs_here && !has_directory && needs_directory
}

/// Newly added agent panels always start a new session. `resume: last` on a
/// preset only governs how existing panels reconnect when they are restored.
/// Continue-style agents (`KiloCode`) keep `last` on the panel because their
/// reconnect flag is applied only on restore, not on the initial launch.
pub(in crate::app) fn normalize_new_panel_resume(options: &mut PanelOptions) {
    if options.kind.supports_session_binding() && matches!(options.resume, PanelResume::Last) {
        options.resume = PanelResume::Fresh;
    }
}

impl HorizonApp {
    pub(in crate::app) fn create_panel(&mut self, ctx: &egui::Context) {
        let workspace_id = self.ensure_workspace_visible(ctx);
        let options = PanelOptions::default();
        match self.create_panel_with_options(options, workspace_id) {
            Ok(panel_id) => self.reveal_new_panel(ctx, workspace_id, panel_id),
            Err(error) => tracing::error!("failed to create panel: {error}"),
        }
    }

    pub(in crate::app) fn create_panel_with_options(
        &mut self,
        options: PanelOptions,
        workspace_id: WorkspaceId,
    ) -> horizon_core::Result<PanelId> {
        self.create_panel_with_options_beside(options, workspace_id, None)
    }

    /// Creates a panel of the person in `workspace_id`, beside the panel
    /// `beside` when one is named.
    ///
    /// This is the path that a person's New buttons, the preset picker and the
    /// canvas picker take. A card's link takes it too: Horizon opens the page in
    /// the browser panel that the person gets from the canvas picker, next to the
    /// card that named it.
    ///
    /// A creation that fails says why in the sidebar, in the words of the tool
    /// or the system that refused it: the person pressed the button and has to
    /// act on that reason. An agent's own panel creates answer the agent
    /// instead, so they do not pass here.
    pub(in crate::app) fn create_panel_with_options_beside(
        &mut self,
        mut options: PanelOptions,
        workspace_id: WorkspaceId,
        beside: Option<PanelId>,
    ) -> horizon_core::Result<PanelId> {
        let workspace_cwd = workspace_cwd(&self.board, workspace_id);
        inherit_workspace_cwd(&mut options, workspace_cwd.as_ref());
        normalize_new_panel_resume(&mut options);
        options.transcript_root.clone_from(&self.transcript_root);
        let kind = options.kind;
        let created = self.placed_options(options, workspace_id).and_then(|options| {
            match beside.filter(|panel| self.board.panel(*panel).is_some()) {
                Some(beside) => self.board.create_panel_beside_waiting(options, workspace_id, beside),
                None => self.board.create_panel_waiting(options, workspace_id),
            }
        });
        if let Err(error) = &created {
            self.notice_panel_creation_failure(kind, error);
        }
        if let Ok(panel_id) = created
            && kind == horizon_core::PanelKind::Browser
        {
            // An engine that cannot start ends its panel at once, and the
            // cleanup then says why this press opened no page.
            self.person_browser_creates.note(panel_id, None);
        }
        created
    }

    /// Says why a panel of `kind` that the person asked for opened nothing.
    ///
    /// A refusal of the place of a workspace names its machine code too, because
    /// that code is what the person reports to the agent that registered the
    /// machine.
    pub(in crate::app) fn notice_panel_creation_failure(
        &mut self,
        kind: horizon_core::PanelKind,
        error: &horizon_core::Error,
    ) {
        let reason = match error {
            horizon_core::Error::Place(machine) => format!("{machine} ({})", machine.code()),
            other => other.to_string(),
        };
        self.set_sidebar_notice(crate::app::creation_notice::panel_creation_notice(kind, &reason));
    }

    /// The options of a new panel of the person with the place of its workspace
    /// applied.
    ///
    /// Every path that a person uses to make a terminal reaches this function:
    /// the New buttons and the shortcut, the preset picker and the canvas
    /// picker, and the directory picker that they open. A terminal of a
    /// workspace that is placed on a machine becomes the SSH panel that runs the
    /// place's own command there; every other panel keeps its own command.
    /// [`horizon_core::place_command`] decides that, so another transport
    /// changes nothing here.
    ///
    /// # Errors
    /// `machine_unavailable` when the machine of the place is gone, and the
    /// refusal of its record when it no longer passes its rule. The creation
    /// that asked says that reason in the sidebar.
    pub(in crate::app) fn placed_options(
        &mut self,
        options: PanelOptions,
        workspace_id: WorkspaceId,
    ) -> horizon_core::Result<PanelOptions> {
        let Some(workspace) = self.board.workspace(workspace_id) else {
            return Ok(options);
        };
        let place = workspace.place.clone();
        if place.is_local() {
            return Ok(options);
        }
        let local_id = workspace.local_id.clone();
        let sessions = self.board.workspace_sessions(workspace_id);
        let sessions = sessions.iter().map(String::as_str).collect::<Vec<_>>();
        let keys = MachineKeys::from_home(self.session_store.home());
        let mut placed = options;
        if let Err(error) = horizon_core::place_command(
            &mut placed,
            &place,
            &self.machine_list,
            &keys,
            &PlaceWorkspace {
                local_id: &local_id,
                sessions: &sessions,
            },
        ) {
            // The creation that the person started says this reason in the
            // sidebar, once, with the kind of the panel.
            return Err(horizon_core::Error::Place(error));
        }
        Ok(placed)
    }

    /// Creates a panel that an agent asked for in its own workspace.
    ///
    /// The panel takes a free place, never on top of another panel, nearest to
    /// `beside`: the agent panel itself, or the agent's previous card.
    pub(in crate::app) fn create_agent_child_panel(
        &mut self,
        options: PanelOptions,
        workspace: WorkspaceId,
        beside: PanelId,
    ) -> horizon_core::Result<PanelId> {
        self.board.create_panel_beside_waiting(options, workspace, beside)
    }

    /// Reveal a panel the same way a sidebar panel-row click does: detached
    /// workspaces get their OS window focused, attached canvases pan/zoom so
    /// the panel is visible.
    pub(in crate::app) fn reveal_selected_panel(&mut self, ctx: &egui::Context, panel_id: PanelId) {
        match self.board.panel_workspace_id(panel_id) {
            Some(workspace_id) => self.reveal_new_panel(ctx, workspace_id, panel_id),
            None => self.board.focus(panel_id),
        }
    }

    /// Reveal a newly created panel the same way the sidebar reveals a
    /// selected panel: detached workspaces get their OS window focused, the
    /// panel re-focused so the helper stays self-contained, and attached
    /// canvases pan/zoom to the panel.
    pub(in crate::app) fn reveal_new_panel(
        &mut self,
        ctx: &egui::Context,
        workspace_id: WorkspaceId,
        panel_id: PanelId,
    ) {
        if self.focus_workspace_window(ctx, workspace_id) {
            self.board.focus(panel_id);
            return;
        }
        self.reveal_panel_visible(ctx, panel_id);
    }

    pub(in crate::app) fn close_panel(&mut self, panel_id: PanelId) {
        if let Some(signal) = self.close_panel_returning_teardown(panel_id) {
            self.board.retire_browser_shutdown_signal(signal);
        }
    }

    /// Close a panel and hand back its browser teardown signal so the caller
    /// can report when the session is really gone; see
    /// [`horizon_core::Board::close_panel_returning_teardown`].
    #[must_use]
    pub(in crate::app) fn close_panel_returning_teardown(
        &mut self,
        panel_id: PanelId,
    ) -> Option<horizon_core::browser::BrowserShutdownSignal> {
        self.refresh_remote_recovery_scope();
        let transcript = self.board.panel(panel_id).and_then(|panel| {
            let transcript = PanelTranscript::for_panel(panel.kind, self.transcript_root.clone(), &panel.local_id)?;
            Some((panel.local_id.clone(), transcript))
        });
        // The panel is gone after this call, so its session is asked for first.
        self.end_machine_session(panel_id);
        self.reconnects.cancel(panel_id);
        let teardown = self.board.close_panel_returning_teardown(panel_id);
        // A buffer that the closed panel held writes its text on the
        // coordination worker, in the order of the writes of that file.
        self.queue_editor_saves();
        self.panel_render_caches.terminal_grid_cache.remove(&panel_id);
        self.panel_render_caches.editor_preview_cache.remove(&panel_id);
        self.panel_render_caches.browser_ui_state.remove(&panel_id);
        self.panel_render_caches.device_ui_state.remove(&panel_id);
        if let Some((local_id, transcript)) = transcript {
            self.delete_closed_transcript(&local_id, transcript);
        }
        teardown
    }

    pub(in crate::app) fn close_workspace_panels(&mut self, workspace_id: WorkspaceId) {
        self.refresh_remote_recovery_scope();
        let panels_to_close: Vec<_> = self
            .board
            .workspace(workspace_id)
            .map(|workspace| {
                workspace
                    .panels
                    .iter()
                    .filter_map(|panel_id| {
                        self.board.panel(*panel_id).map(|panel| {
                            (
                                panel.local_id.clone(),
                                PanelTranscript::for_panel(panel.kind, self.transcript_root.clone(), &panel.local_id),
                            )
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        // The sessions are asked for while their panels still name them.
        let machine_panels = self
            .board
            .workspace(workspace_id)
            .map(|workspace| workspace.panels.clone())
            .unwrap_or_default();
        self.end_machine_sessions(machine_panels);

        if panels_to_close.is_empty() {
            self.board.close_panels_in_workspace(workspace_id);
            self.queue_editor_saves();
            return;
        }

        let closed_panel_ids = self.board.close_panels_in_workspace(workspace_id);
        // The buffers that the closed panels held write their text on the
        // coordination worker, in the order of the writes of their files.
        self.queue_editor_saves();
        for panel_id in &closed_panel_ids {
            self.panel_screen_rects.remove(panel_id);
            self.terminal_body_screen_rects.remove(panel_id);
            self.panel_render_caches.terminal_grid_cache.remove(panel_id);
            self.panel_render_caches.editor_preview_cache.remove(panel_id);
            self.panel_render_caches.browser_ui_state.remove(panel_id);
            self.panel_render_caches.device_ui_state.remove(panel_id);
        }

        if self
            .renaming_panel
            .is_some_and(|panel_id| closed_panel_ids.contains(&panel_id))
        {
            self.clear_panel_rename();
        }

        for (local_id, transcript) in panels_to_close {
            if let Some(transcript) = transcript {
                self.delete_closed_transcript(&local_id, transcript);
            }
        }
    }

    pub(in crate::app) fn clear_workspace_rename(&mut self) {
        self.renaming_workspace = None;
        self.rename_buffer.clear();
    }

    pub(in crate::app) fn clear_panel_rename(&mut self) {
        self.renaming_panel = None;
        self.panel_rename_buffer.clear();
    }

    pub(in crate::app) fn add_panel_to_workspace(
        &mut self,
        ctx: &egui::Context,
        workspace_id: WorkspaceId,
        preset: PresetConfig,
        canvas_pos: Option<[f32; 2]>,
    ) {
        // A directory that was renamed or deleted is no directory for a new
        // panel: ask where to run instead of starting it in Horizon's own
        // checkout. Picking one also repairs the workspace directory.
        let has_directory = workspace_cwd(&self.board, workspace_id).is_some_and(|path| path.is_dir());
        let runs_here = self
            .board
            .workspace(workspace_id)
            .is_none_or(|workspace| workspace.place.is_local());
        if asks_where_to_run(runs_here, has_directory, preset.requires_workspace_cwd()) {
            self.open_panel_dir_picker(workspace_id, preset, canvas_pos);
        } else {
            let mut options = preset_panel_options(&preset, &self.template_config.browser);
            options.position = add_panel_position(&self.board, workspace_id, canvas_pos);
            match self.create_panel_with_options(options, workspace_id) {
                Ok(panel_id) => self.reveal_new_panel(ctx, workspace_id, panel_id),
                Err(error) => tracing::error!("failed to create panel: {error}"),
            }
            self.mark_runtime_dirty();
        }
    }

    pub(in crate::app) fn open_panel_dir_picker(
        &mut self,
        workspace_id: WorkspaceId,
        preset: PresetConfig,
        canvas_pos: Option<[f32; 2]>,
    ) {
        let workspace_cwd = workspace_cwd(&self.board, workspace_id);
        self.dir_picker = Some(DirPicker::with_seed(
            DirPickerPurpose::AddPanel {
                workspace_id,
                preset,
                canvas_pos,
            },
            workspace_cwd.as_deref(),
        ));
    }
}

#[cfg(test)]
mod tests {
    use horizon_core::{PanelKind, PanelOptions, PanelResume, PresetConfig, RuntimeState, StartupDecision};

    use super::normalize_new_panel_resume;

    #[test]
    fn new_agent_panels_always_start_fresh_sessions() {
        for kind in [
            PanelKind::Claude,
            PanelKind::Codex,
            PanelKind::OpenCode,
            PanelKind::Pi,
            PanelKind::Grok,
        ] {
            let mut options = PanelOptions {
                kind,
                resume: PanelResume::Last,
                ..PanelOptions::default()
            };

            normalize_new_panel_resume(&mut options);

            assert_eq!(options.resume, PanelResume::Fresh, "kind {kind:?}");
        }
    }

    #[test]
    fn continue_style_agents_keep_last_resume() {
        let mut options = PanelOptions {
            kind: PanelKind::KiloCode,
            resume: PanelResume::Last,
            ..PanelOptions::default()
        };

        normalize_new_panel_resume(&mut options);

        assert_eq!(options.resume, PanelResume::Last);
    }

    #[test]
    fn explicitly_requested_sessions_are_preserved() {
        let mut options = PanelOptions {
            kind: PanelKind::Claude,
            resume: PanelResume::Session {
                session_id: "session-1".to_string(),
            },
            ..PanelOptions::default()
        };

        normalize_new_panel_resume(&mut options);

        assert_eq!(
            options.resume,
            PanelResume::Session {
                session_id: "session-1".to_string(),
            }
        );
    }

    mod close;
    mod placed;

    fn shell_preset() -> PresetConfig {
        PresetConfig {
            name: "shell".to_string(),
            alias: None,
            kind: PanelKind::Shell,
            command: None,
            args: Vec::new(),
            resume: PanelResume::Fresh,
            ssh_connection: None,
        }
    }

    mod workspace_directory {
        use horizon_core::{PresetConfig, RuntimeState, StartupDecision};

        use crate::app::HorizonApp;
        use crate::app::test_support::test_app_with_startup;

        use super::shell_preset;

        fn running_app() -> (tempfile::TempDir, egui::Context, HorizonApp) {
            test_app_with_startup(StartupDecision::Ephemeral {
                runtime_state: Box::new(RuntimeState::default()),
            })
        }

        fn exiting_shell() -> PresetConfig {
            PresetConfig {
                command: Some("/bin/sh".to_string()),
                args: vec!["-c".to_string(), "exit".to_string()],
                ..shell_preset()
            }
        }

        #[test]
        fn a_directory_that_is_gone_asks_where_the_workspace_runs() {
            let (temp, ctx, mut app) = running_app();
            let workspace = app.board.create_workspace("moved");
            let gone = temp.path().join("renamed-away");
            app.board.workspace_mut(workspace).expect("workspace").cwd = Some(gone);

            app.add_panel_to_workspace(&ctx, workspace, exiting_shell(), None);

            assert!(
                app.dir_picker.is_some(),
                "a new panel asks where to run instead of starting in Horizon's own directory"
            );
            assert!(app.board.panels.is_empty(), "no panel starts anywhere else");
        }

        #[test]
        fn an_existing_directory_starts_the_panel_without_asking() {
            let (temp, ctx, mut app) = running_app();
            let workspace = app.board.create_workspace("here");
            app.board.workspace_mut(workspace).expect("workspace").cwd = Some(temp.path().to_path_buf());

            app.add_panel_to_workspace(&ctx, workspace, exiting_shell(), None);

            assert!(app.dir_picker.is_none(), "the workspace directory is usable");
            let panel_id = *app.board.panels.first().map(|panel| &panel.id).expect("panel");
            assert_eq!(
                app.board.panel(panel_id).and_then(|panel| panel.launch_cwd.clone()),
                Some(temp.path().to_path_buf())
            );
            app.board.shutdown_terminal_panels();
        }
    }

    #[test]
    fn new_panel_reveals_it_when_the_view_is_panned_away() {
        use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app_with_startup};

        let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        // Establish the viewport so canvas math matches the assertion below.
        run_app_frame_with_input(&ctx, &mut app, raw_input([1600.0, 1000.0], Some([0.0, 0.0])));

        let workspace_id = app.board.create_workspace("far");
        {
            let workspace = app.board.workspace_mut(workspace_id).expect("workspace");
            workspace.position = [6000.0, 4000.0];
            workspace.cwd = Some(std::env::temp_dir());
        }
        app.add_panel_to_workspace(&ctx, workspace_id, shell_preset(), None);

        // The user pans the canvas away from the workspace.
        app.canvas_view.set_pan_offset([0.0, 0.0]);

        app.add_panel_to_workspace(&ctx, workspace_id, shell_preset(), None);
        let new_panel = app
            .board
            .workspace(workspace_id)
            .and_then(|workspace| workspace.panels.last().copied())
            .expect("new panel");

        // Like a sidebar selection: the view moves to the new panel and the
        // panel is focused.
        let pan_moved = app.canvas_view.pan_offset[0].abs() + app.canvas_view.pan_offset[1].abs();
        assert!(pan_moved > 1.0, "view should have panned to the new panel");
        assert_eq!(app.board.focused, Some(new_panel));

        run_app_frame_with_input(&ctx, &mut app, raw_input([1600.0, 1000.0], Some([0.0, 0.0])));
        let canvas_rect = app.canvas_rect(&ctx);
        let panel = app.board.panel(new_panel).expect("panel");
        let screen_min = app.canvas_to_screen(
            canvas_rect,
            egui::Pos2::new(panel.layout.position[0], panel.layout.position[1]),
        );
        let screen_rect = egui::Rect::from_min_size(
            screen_min,
            app.canvas_size_to_screen(egui::Vec2::new(panel.layout.size[0], panel.layout.size[1])),
        );
        assert!(
            canvas_rect.intersects(screen_rect),
            "new panel should be visible after creation: panel screen {screen_rect:?} vs canvas {canvas_rect:?}"
        );
    }

    #[test]
    fn navigation_reveals_panel_after_the_view_is_panned_away() {
        use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app_with_startup};

        let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        let viewport = [1600.0, 1000.0];
        run_app_frame_with_input(&ctx, &mut app, raw_input(viewport, Some([0.0, 0.0])));

        let workspace_id = seeded_workspace(&mut app, "click-reveal");
        app.add_panel_to_workspace(&ctx, workspace_id, shell_preset(), None);
        let panel_id = app
            .board
            .workspace(workspace_id)
            .and_then(|workspace| workspace.panels.last().copied())
            .expect("panel");
        let reveal_pan = app.canvas_view.pan_offset;

        app.canvas_view
            .set_pan_offset([reveal_pan[0] + 120.0, reveal_pan[1] + 80.0]);
        run_app_frame_with_input(&ctx, &mut app, raw_input(viewport, Some([0.0, 0.0])));
        app.reveal_selected_panel(&ctx, panel_id);

        assert_eq!(app.board.focused, Some(panel_id));
        assert_pan_near(
            app.canvas_view.pan_offset,
            reveal_pan,
            "navigation should restore the reveal pan",
        );
    }

    #[test]
    fn titlebar_clicks_focus_and_rename_without_changing_the_view() {
        use crate::app::test_support::{
            editor_workspace_state, raw_input, run_app_frame_with_input, test_app_with_startup,
        };

        for zoom in [0.75, 1.0, 1.5] {
            let mut workspace = editor_workspace_state("header-click", [0.0, 0.0]);
            workspace.panels[0].size = Some([2200.0, 1400.0]);
            let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
                runtime_state: Box::new(RuntimeState {
                    workspaces: vec![workspace],
                    ..RuntimeState::default()
                }),
            });
            let viewport = [1600.0, 1000.0];
            run_app_frame_with_input(&ctx, &mut app, raw_input(viewport, Some([0.0, 0.0])));
            let panel_id = app.board.workspaces[0].panels[0];
            app.board.focused = None;
            app.canvas_view.set_zoom(zoom);
            app.canvas_view.set_pan_offset([140.0, 100.0]);
            app.pan_target = None;
            let view = app.canvas_view;
            run_app_frame_with_input(&ctx, &mut app, raw_input(viewport, Some([0.0, 0.0])));
            let titlebar = titlebar_click_pos(&app, panel_id);

            click_screen_pos(&ctx, &mut app, viewport, titlebar);
            assert_eq!(app.board.focused, Some(panel_id));
            assert_eq!(app.canvas_view, view, "first click must preserve the view");
            assert_eq!(app.renaming_panel, None);

            click_screen_pos(&ctx, &mut app, viewport, titlebar);
            assert_eq!(app.renaming_panel, Some(panel_id));
            assert_eq!(app.canvas_view, view, "double-click must preserve the view");

            run_app_frame_with_input(&ctx, &mut app, raw_input(viewport, Some([0.0, 0.0])));
            "Renamed panel".clone_into(&mut app.panel_rename_buffer);
            let mut enter = raw_input(viewport, Some([0.0, 0.0]));
            enter.events.push(egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
            run_app_frame_with_input(&ctx, &mut app, enter);
            assert_eq!(app.board.panel(panel_id).expect("panel").title, "Renamed panel");
            assert_eq!(app.renaming_panel, None);
            assert_eq!(app.canvas_view, view, "committing the name must preserve the view");
        }
    }

    #[test]
    fn panel_body_click_focuses_without_revealing() {
        use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app_with_startup};

        let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        let viewport = [1600.0, 1000.0];
        run_app_frame_with_input(&ctx, &mut app, raw_input(viewport, Some([0.0, 0.0])));

        let workspace_id = seeded_workspace(&mut app, "body-click");
        app.add_panel_to_workspace(&ctx, workspace_id, shell_preset(), None);
        let panel_id = app
            .board
            .workspace(workspace_id)
            .and_then(|workspace| workspace.panels.last().copied())
            .expect("panel");
        let reveal_pan = app.canvas_view.pan_offset;

        let nudged = [reveal_pan[0] + 120.0, reveal_pan[1] + 80.0];
        app.canvas_view.set_pan_offset(nudged);
        run_app_frame_with_input(&ctx, &mut app, raw_input(viewport, Some([0.0, 0.0])));
        let body = body_click_pos(&app, panel_id);
        click_screen_pos(&ctx, &mut app, viewport, body);

        assert_eq!(app.board.focused, Some(panel_id));
        assert_pan_near(
            app.canvas_view.pan_offset,
            nudged,
            "body click should keep the nudged pan instead of restoring the sidebar reveal pose",
        );
        assert!(
            !pan_near(app.canvas_view.pan_offset, reveal_pan),
            "body click must not snap back to the reveal pan {reveal_pan:?}"
        );
    }

    #[test]
    fn reveal_selected_panel_does_not_pan_the_root_canvas_for_detached_workspaces() {
        use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app_with_startup};

        let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        run_app_frame_with_input(&ctx, &mut app, raw_input([1600.0, 1000.0], Some([0.0, 0.0])));

        let workspace_id = seeded_workspace(&mut app, "detached-reveal");
        app.add_panel_to_workspace(&ctx, workspace_id, shell_preset(), None);
        let panel_id = app
            .board
            .workspace(workspace_id)
            .and_then(|workspace| workspace.panels.last().copied())
            .expect("panel");
        let reveal_pan = app.canvas_view.pan_offset;
        app.detach_workspace(workspace_id);

        let nudged = [reveal_pan[0] + 120.0, reveal_pan[1] + 80.0];
        app.canvas_view.set_pan_offset(nudged);
        app.reveal_selected_panel(&ctx, panel_id);

        assert_eq!(app.board.focused, Some(panel_id));
        assert_pan_near(
            app.canvas_view.pan_offset,
            nudged,
            "detached reveal should focus the panel without panning the root canvas",
        );
    }

    fn seeded_workspace(app: &mut crate::app::HorizonApp, name: &str) -> horizon_core::WorkspaceId {
        let workspace_id = app.board.create_workspace(name);
        {
            let workspace = app.board.workspace_mut(workspace_id).expect("workspace");
            workspace.cwd = Some(std::env::temp_dir());
        }
        workspace_id
    }

    fn titlebar_click_pos(app: &crate::app::HorizonApp, panel_id: horizon_core::PanelId) -> egui::Pos2 {
        let rect = app
            .panel_screen_rects
            .get(&panel_id)
            .copied()
            .expect("panel screen rect");
        egui::Pos2::new(
            rect.min.x + 80.0,
            rect.min.y + super::super::super::PANEL_TITLEBAR_HEIGHT * 0.5,
        )
    }

    fn body_click_pos(app: &crate::app::HorizonApp, panel_id: horizon_core::PanelId) -> egui::Pos2 {
        let rect = app
            .panel_screen_rects
            .get(&panel_id)
            .copied()
            .expect("panel screen rect");
        egui::Pos2::new(
            rect.min.x + 80.0,
            rect.min.y + super::super::super::PANEL_TITLEBAR_HEIGHT + 40.0,
        )
    }

    fn pan_near(actual: [f32; 2], expected: [f32; 2]) -> bool {
        (actual[0] - expected[0]).abs() < 1.0 && (actual[1] - expected[1]).abs() < 1.0
    }

    fn assert_pan_near(actual: [f32; 2], expected: [f32; 2], message: &str) {
        assert!(pan_near(actual, expected), "{message}: {expected:?} vs {actual:?}");
    }

    fn click_screen_pos(ctx: &egui::Context, app: &mut crate::app::HorizonApp, viewport: [f32; 2], pos: egui::Pos2) {
        use crate::app::test_support::run_app_frame_with_input;

        run_app_frame_with_input(ctx, app, pointer_input(viewport, pos, None));
        run_app_frame_with_input(ctx, app, pointer_input(viewport, pos, Some(true)));
        run_app_frame_with_input(ctx, app, pointer_input(viewport, pos, Some(false)));
    }

    fn pointer_input(viewport: [f32; 2], pos: egui::Pos2, pressed: Option<bool>) -> egui::RawInput {
        use crate::app::test_support::raw_input;

        let mut input = raw_input(viewport, Some([0.0, 0.0]));
        input.events.push(egui::Event::PointerMoved(pos));
        if let Some(pressed) = pressed {
            input.events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        input
    }
}
