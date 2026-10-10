use horizon_core::WorkspaceId;

use super::HorizonApp;

#[cfg(target_os = "linux")]
pub(super) fn resolve_native_picker_drops(ctx: &egui::Context, observed: &crate::input::ObservedKeyboardInputs) {
    let dropped = ctx.input_mut(|input| std::mem::take(&mut input.raw.dropped_files));
    if dropped.is_empty() {
        return;
    }
    let mut resolved: Vec<_> = observed
        .take_native_drop_batches(ctx.viewport_id(), &dropped)
        .into_iter()
        .flat_map(|(files, _)| files)
        .collect();
    resolved.extend(
        dropped
            .into_iter()
            .filter(|file| !crate::input::ObservedKeyboardInputs::is_native_drop_token(file.path())),
    );
    ctx.input_mut(|input| input.raw.dropped_files = resolved);
}

impl HorizonApp {
    pub(super) fn browser_file_chooser_open(&self) -> bool {
        self.board.panels.iter().any(|panel| {
            panel
                .browser()
                .is_some_and(|browser| browser.frame_slot.file_chooser().request().is_some())
        })
    }

    pub(super) fn host_dialog_open(&self) -> bool {
        self.host_content_dialog_open()
    }

    pub(super) fn host_content_dialog_open(&self) -> bool {
        if self.saved_session_deletion.has_notice() {
            return true;
        }
        #[cfg(feature = "cloud-workspaces")]
        if self.cloud_close_confirmation_open() || self.idle_stop_open() {
            return true;
        }
        self.cloud_creation_open() || self.browser_file_chooser_open()
    }

    pub(super) fn render_browser_file_chooser(&mut self, ctx: &egui::Context, workspace: Option<WorkspaceId>) {
        let candidate = self.board.panels.iter().find_map(|panel| {
            let in_viewport = workspace.map_or_else(
                || {
                    self.board
                        .workspace(panel.workspace_id)
                        .is_some_and(|workspace| !self.detached_workspaces.contains_key(&workspace.local_id))
                },
                |workspace| panel.workspace_id == workspace,
            );
            let browser = panel.browser()?;
            (in_viewport && browser.frame_slot.file_chooser().request().is_some())
                .then(|| (panel.id, browser.frame_slot.file_chooser().clone()))
        });
        let Some((panel, handle)) = candidate else {
            return;
        };
        #[cfg(target_os = "linux")]
        if self.observed_keyboard_inputs.is_wayland_backend() {
            resolve_native_picker_drops(ctx, &self.observed_keyboard_inputs);
        }
        if self
            .panel_render_caches
            .browser_ui_state
            .entry(panel)
            .or_default()
            .show_file_chooser(ctx, panel, &handle)
        {
            self.consume_navigation_key(
                ctx,
                horizon_core::ShortcutBinding::new(
                    horizon_core::ShortcutModifiers::NONE,
                    horizon_core::ShortcutKey::Escape,
                ),
            );
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::app::test_support;

    #[test]
    fn detached_picker_resolves_its_current_viewport_native_batch() {
        use crate::test_egui::DiscardTextures;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Overview.pdf");
        std::fs::write(&path, b"synthetic detached picker bytes").unwrap();
        let observed = crate::input::ObservedKeyboardInputs::default();
        observed.set_wayland_backend(true);
        observed.native_window_seen(20);
        let token = observed
            .native_drop_position(20, [300.0, 200.0], vec![path.clone()])
            .unwrap();
        let viewport = egui::ViewportId::from_hash_of("detached-upload");
        let mut input = egui::RawInput {
            viewport_id: viewport,
            dropped_files: vec![test_support::dropped_file(token.clone())],
            ..Default::default()
        };
        input.viewports.insert(viewport, egui::ViewportInfo::default());
        let ctx = egui::Context::default();
        let _ = ctx
            .run_ui(input, |ui| {
                assert_eq!(ui.ctx().viewport_id(), viewport);
                resolve_native_picker_drops(ui.ctx(), &observed);
                let files = ui.ctx().input(|input| input.raw.dropped_files.clone());
                assert_eq!(files.len(), 1);
                assert_eq!(files[0].path(), path);
                assert_eq!(files[0].bytes().unwrap(), b"synthetic detached picker bytes");
            })
            .discard_textures();
        assert!(
            observed
                .take_native_drop_batches(viewport, &[test_support::dropped_file(token)])
                .is_empty()
        );
    }

    #[test]
    fn picker_resolves_native_tokens_before_drop_state_is_discarded() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Overview.pdf");
        std::fs::write(&path, b"synthetic picker bytes").unwrap();
        let observed = crate::input::ObservedKeyboardInputs::default();
        observed.set_wayland_backend(true);
        observed.native_window_seen(10);
        let token = observed
            .native_drop_position(10, [300.0, 200.0], vec![path.clone()])
            .unwrap();
        let ctx = egui::Context::default();
        ctx.input_mut(|input| {
            input.raw.dropped_files = vec![
                test_support::dropped_file(&token),
                test_support::dropped_file("/.horizon-native-drop/999"),
                test_support::dropped_file("/fixture/Notes.txt"),
            ];
        });
        resolve_native_picker_drops(&ctx, &observed);
        let files = ctx.input(|input| input.raw.dropped_files.clone());
        observed.discard_native_drop_positions(&files);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path(), path);
        assert_eq!(files[0].bytes().unwrap(), b"synthetic picker bytes");
        assert_eq!(files[1].path(), std::path::Path::new("/fixture/Notes.txt"));
        assert!(
            observed
                .take_native_drop_batches(egui::ViewportId::ROOT, &[test_support::dropped_file(token)])
                .is_empty()
        );
    }
}
