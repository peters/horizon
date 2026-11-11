//! The saves of editor panels, on the coordination worker.
//!
//! An editor panel keeps its text in memory while the person types, and a save
//! writes the whole file. The frame hands the coordination worker a copy of the
//! text, and the panel clears its dirty mark when the answer says that its file
//! holds that text. A save that fails says why in the panel, and its dirty mark
//! stays.

use horizon_core::EditorSave;

use super::HorizonApp;

impl HorizonApp {
    /// Queues the write of every editor buffer whose save the frame asked for.
    /// The text is a copy, so what the file receives is what the frame showed.
    pub(in crate::app) fn queue_editor_saves(&mut self) {
        for save in self.board.take_editor_saves() {
            let path = save.path.clone();
            let text = save.text.clone();
            self.browser_create_host.io.then(
                move || std::fs::write(&path, &text).map_err(|error| error.to_string()),
                move |app, result| {
                    let result = result.unwrap_or_else(|| Err("the write did not finish".to_owned()));
                    app.apply_editor_save(&save, result);
                },
            );
        }
    }

    fn apply_editor_save(&mut self, save: &EditorSave, result: Result<(), String>) {
        self.board
            .finish_editor_save(save, result.map_err(horizon_core::Error::Editor));
    }
}

#[cfg(test)]
mod tests {
    use horizon_core::{PanelKind, PanelOptions};

    use crate::app::test_support::{run_app_frame, test_app};

    fn editor_options(path: &std::path::Path) -> PanelOptions {
        PanelOptions {
            kind: PanelKind::Editor,
            command: Some(path.display().to_string()),
            ..PanelOptions::default()
        }
    }

    #[test]
    fn an_editor_opens_with_its_text_and_saves_on_the_coordination_worker() {
        let (temp, mut app) = test_app();
        let file = temp.path().join("notes.md");
        std::fs::write(&file, "the saved text\n").expect("the file of the editor");
        let workspace = app.board.ensure_workspace();
        let panel = app
            .create_panel_with_options(editor_options(&file), workspace)
            .expect("an editor panel");

        // The panel opens with an empty buffer and shows the text of its file
        // when the read of the worker answers.
        {
            let _frame = horizon_core::ui_thread::FrameScope::enter();
            assert_eq!(
                app.board
                    .panel(panel)
                    .and_then(horizon_core::Panel::editor)
                    .map(|editor| editor.text.clone()),
                Some(String::new()),
                "the frame that opens the panel reads no file"
            );
        }
        let ctx = egui::Context::default();
        run_app_frame(&ctx, &mut app);
        app.settle_panel_starts();
        assert_eq!(
            app.board
                .panel(panel)
                .and_then(horizon_core::Panel::editor)
                .map(|editor| editor.text.clone()),
            Some("the saved text\n".to_owned()),
            "the panel shows the text that the worker read"
        );

        // The person types and saves: the frame copies the text and the worker
        // writes it.
        {
            let _frame = horizon_core::ui_thread::FrameScope::enter();
            let editor = app
                .board
                .panel_mut(panel)
                .and_then(horizon_core::Panel::editor_mut)
                .expect("the editor of the panel");
            editor.text = "the new text\n".to_owned();
            editor.dirty = true;
            editor.request_save();
            app.queue_editor_saves();
            assert_eq!(
                app.board
                    .panel(panel)
                    .and_then(horizon_core::Panel::editor)
                    .map(|editor| (editor.saving, editor.dirty)),
                Some((true, true)),
                "the buffer waits for its write and keeps its dirty mark"
            );
        }
        app.settle_browser_host_io();
        assert_eq!(
            std::fs::read_to_string(&file).expect("the file of the editor"),
            "the new text\n",
            "the coordination worker wrote the text of the frame"
        );
        assert_eq!(
            app.board
                .panel(panel)
                .and_then(horizon_core::Panel::editor)
                .map(|editor| (editor.saving, editor.dirty)),
            Some((false, false)),
            "the dirty mark clears with the answer of the write"
        );

        // A write that fails keeps the dirty mark and says why in the panel.
        std::fs::remove_file(&file).expect("the file of the editor");
        std::fs::create_dir(&file).expect("a directory where the file was");
        {
            let _frame = horizon_core::ui_thread::FrameScope::enter();
            let editor = app
                .board
                .panel_mut(panel)
                .and_then(horizon_core::Panel::editor_mut)
                .expect("the editor of the panel");
            editor.text = "the text that cannot be written\n".to_owned();
            editor.dirty = true;
            editor.request_save();
            app.queue_editor_saves();
        }
        app.settle_browser_host_io();
        let editor = app
            .board
            .panel(panel)
            .and_then(horizon_core::Panel::editor)
            .expect("the editor of the panel");
        assert!(editor.dirty, "a failed save keeps the dirty mark");
        assert!(!editor.saving, "the save ended");
        let error = editor.error.clone().expect("the panel says why the save failed");
        assert!(error.starts_with("Could not save this file"), "{error}");
    }
}
