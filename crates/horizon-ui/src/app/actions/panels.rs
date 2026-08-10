use crate::app::HorizonApp;
use crate::app::settings::{SpeechSetupRequest, speech_setup_prompt};
use crate::dir_picker::{DirPicker, DirPickerPurpose};
use horizon_core::{Error, PanelId, PanelOptions, PanelResume, PanelTranscript, PresetConfig, WorkspaceId};

use super::{add_panel_position, inherit_workspace_cwd, workspace_cwd};

fn speech_setup_panel_options(
    request: &SpeechSetupRequest,
    config_path: &std::path::Path,
) -> horizon_core::Result<PanelOptions> {
    if request.preset.kind != request.agent.panel_kind() {
        return Err(Error::Config(format!(
            "speech setup request for {} used a mismatched {:?} preset",
            request.agent.display_name(),
            request.preset.kind
        )));
    }

    let mut options = request.preset.to_panel_options();
    options.name = Some(request.agent.panel_title());
    options.resume = PanelResume::Fresh;
    options.session_binding = None;
    options.initial_agent_prompt = Some(speech_setup_prompt(request.agent, config_path));
    options.agent_login_shell = true;
    Ok(options)
}

/// Newly added agent panels always start a new session. `resume: last` on a
/// preset only governs how existing panels reconnect when they are restored.
/// Continue-style agents (`KiloCode`) keep `last` on the panel because their
/// reconnect flag is applied only on restore, not on the initial launch.
fn normalize_new_panel_resume(options: &mut PanelOptions) {
    if options.kind.supports_session_binding() && matches!(options.resume, PanelResume::Last) {
        options.resume = PanelResume::Fresh;
    }
}

impl HorizonApp {
    pub(in crate::app) fn create_panel(&mut self, ctx: &egui::Context) {
        let workspace_id = self.ensure_workspace_visible(ctx);
        if let Err(error) = self.create_panel_with_options(PanelOptions::default(), workspace_id) {
            tracing::error!("failed to create panel: {error}");
        }
    }

    pub(in crate::app) fn create_panel_with_options(
        &mut self,
        mut options: PanelOptions,
        workspace_id: WorkspaceId,
    ) -> horizon_core::Result<PanelId> {
        let workspace_cwd = workspace_cwd(&self.board, workspace_id);
        inherit_workspace_cwd(&mut options, workspace_cwd.as_ref());
        normalize_new_panel_resume(&mut options);
        options.transcript_root.clone_from(&self.transcript_root);
        self.board.create_panel(options, workspace_id)
    }

    pub(in crate::app) fn launch_speech_setup_agent(
        &mut self,
        ctx: &egui::Context,
        request: &SpeechSetupRequest,
    ) -> horizon_core::Result<PanelId> {
        let setup_cwd = self
            .board
            .active_workspace
            .and_then(|workspace_id| workspace_cwd(&self.board, workspace_id));
        request.verify_command(setup_cwd.as_deref()).map_err(Error::Config)?;
        let workspace_id = self.ensure_workspace_visible(ctx);
        let mut options = speech_setup_panel_options(request, &self.config_path)?;
        options.position = add_panel_position(&self.board, workspace_id, None);

        let panel_id = self.create_panel_with_options(options, workspace_id)?;
        if !self.focus_panel_in_workspace_window(ctx, workspace_id, panel_id) {
            self.focus_panel_visible(ctx, panel_id, false);
            self.pending_terminal_focus = Some(super::super::PendingTerminalFocus {
                panel_id,
                viewport_id: egui::ViewportId::ROOT,
            });
        }
        self.mark_runtime_dirty();
        Ok(panel_id)
    }

    pub(in crate::app) fn close_panel(&mut self, panel_id: PanelId) {
        let transcript = self
            .board
            .panel(panel_id)
            .and_then(|panel| PanelTranscript::for_panel(panel.kind, self.transcript_root.clone(), &panel.local_id));
        self.board.close_panel(panel_id);
        if self
            .pending_terminal_focus
            .is_some_and(|pending| pending.panel_id == panel_id)
        {
            self.pending_terminal_focus = None;
        }
        self.terminal_grid_cache.remove(&panel_id);
        self.editor_preview_cache.remove(&panel_id);
        if let Some(transcript) = transcript
            && let Err(error) = transcript.delete_all()
        {
            tracing::warn!(panel_id = panel_id.0, "failed to delete panel transcript: {error}");
        }
    }

    pub(in crate::app) fn close_workspace_panels(&mut self, workspace_id: WorkspaceId) {
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
                                *panel_id,
                                PanelTranscript::for_panel(panel.kind, self.transcript_root.clone(), &panel.local_id),
                            )
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        if panels_to_close.is_empty() {
            self.board.close_panels_in_workspace(workspace_id);
            return;
        }

        let closed_panel_ids = self.board.close_panels_in_workspace(workspace_id);
        if self
            .pending_terminal_focus
            .is_some_and(|pending| closed_panel_ids.contains(&pending.panel_id))
        {
            self.pending_terminal_focus = None;
        }
        for panel_id in &closed_panel_ids {
            self.panel_screen_rects.remove(panel_id);
            self.terminal_body_screen_rects.remove(panel_id);
            self.terminal_grid_cache.remove(panel_id);
            self.editor_preview_cache.remove(panel_id);
        }

        if self
            .renaming_panel
            .is_some_and(|panel_id| closed_panel_ids.contains(&panel_id))
        {
            self.clear_panel_rename();
        }

        for (panel_id, transcript) in panels_to_close {
            if let Some(transcript) = transcript
                && let Err(error) = transcript.delete_all()
            {
                tracing::warn!(panel_id = panel_id.0, "failed to delete panel transcript: {error}");
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
        workspace_id: WorkspaceId,
        preset: PresetConfig,
        canvas_pos: Option<[f32; 2]>,
    ) {
        if workspace_cwd(&self.board, workspace_id).is_some() || !preset.requires_workspace_cwd() {
            let mut options = preset.to_panel_options();
            options.position = add_panel_position(&self.board, workspace_id, canvas_pos);
            if let Err(error) = self.create_panel_with_options(options, workspace_id) {
                tracing::error!("failed to create panel: {error}");
            }
            self.mark_runtime_dirty();
        } else {
            self.open_panel_dir_picker(workspace_id, preset, canvas_pos);
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
    use std::path::Path;

    use horizon_core::{PanelKind, PanelOptions, PanelResume, PresetConfig};

    use super::{normalize_new_panel_resume, speech_setup_panel_options};
    use crate::app::settings::{SpeechSetupAgent, SpeechSetupRequest};

    fn setup_preset(kind: PanelKind) -> PresetConfig {
        PresetConfig {
            name: "Custom setup agent".to_string(),
            alias: None,
            kind,
            command: Some("/opt/tools/setup-agent".to_string()),
            args: vec!["--safe-default".to_string()],
            resume: PanelResume::Session {
                session_id: "must-not-resume".to_string(),
            },
            ssh_connection: None,
        }
    }

    #[test]
    fn new_agent_panels_always_start_fresh_sessions() {
        for kind in [PanelKind::Claude, PanelKind::Codex, PanelKind::OpenCode, PanelKind::Pi] {
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

    #[test]
    fn speech_setup_options_use_the_selected_preset_but_force_a_fresh_one_shot_panel() {
        let request = SpeechSetupRequest {
            agent: SpeechSetupAgent::Claude,
            preset: setup_preset(PanelKind::Claude),
        };

        let options = speech_setup_panel_options(&request, Path::new("/users/alice/.horizon/config.yaml"))
            .expect("matching setup request");

        assert_eq!(options.name.as_deref(), Some("Speech Input Setup — Claude"));
        assert_eq!(options.kind, PanelKind::Claude);
        assert_eq!(options.command.as_deref(), Some("/opt/tools/setup-agent"));
        assert_eq!(options.args, ["--safe-default"]);
        assert_eq!(options.resume, PanelResume::Fresh);
        assert!(options.session_binding.is_none());
        assert!(options.agent_login_shell);
        let prompt = options.initial_agent_prompt.expect("one-shot prompt");
        assert!(prompt.contains("/users/alice/.horizon/config.yaml"));
        assert!(!options.args.iter().any(|argument| argument.contains("Speech Input")));
    }

    #[test]
    fn speech_setup_options_reject_a_mismatched_preset_kind() {
        let request = SpeechSetupRequest {
            agent: SpeechSetupAgent::Codex,
            preset: setup_preset(PanelKind::Claude),
        };

        let Err(error) = speech_setup_panel_options(&request, Path::new("/tmp/config.yaml")) else {
            panic!("mismatched setup preset must be rejected");
        };

        assert!(error.to_string().contains("mismatched Claude preset"));
    }
}
