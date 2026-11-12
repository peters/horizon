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
        let result = result.map_err(horizon_core::Error::Editor);
        if self.browser_create_host.io.applying_stale() {
            // The board that asked for this write is gone, and a panel of the
            // board that stands now can carry the same id. The file keeps the
            // text, and a failure still leaves its line.
            if let Err(error) = result {
                self.set_sidebar_sticky_notice(format!("Could not save {}: {error}", save.path.display()));
            }
            return;
        }
        if let Some(line) = self.board.finish_editor_save(save, result) {
            self.set_sidebar_sticky_notice(line);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use horizon_core::{Panel, PanelId, PanelKind, PanelOptions, RuntimeState};

    use crate::app::test_support::{run_app_frame, test_app};

    fn editor_options(path: &std::path::Path) -> PanelOptions {
        PanelOptions {
            kind: PanelKind::Editor,
            command: Some(path.display().to_string()),
            ..PanelOptions::default()
        }
    }

    /// A panel of `file`, whose buffer holds `text` and is dirty, as a person
    /// leaves it when they type and close the panel.
    fn dirty_editor(app: &mut crate::app::HorizonApp, file: &Path, text: &str) -> PanelId {
        let workspace = app.board.ensure_workspace();
        let panel = app
            .create_panel_with_options(editor_options(file), workspace)
            .expect("an editor panel");
        let ctx = egui::Context::default();
        run_app_frame(&ctx, app);
        app.settle_panel_starts();
        let editor = app
            .board
            .panel_mut(panel)
            .and_then(Panel::editor_mut)
            .expect("the editor of the panel");
        text.clone_into(&mut editor.text);
        editor.dirty = true;
        panel
    }

    fn closed_editor_text(app: &crate::app::HorizonApp, panel: PanelId) -> Option<(bool, bool)> {
        app.board
            .panel(panel)
            .and_then(Panel::editor)
            .map(|editor| (editor.saving, editor.dirty))
    }

    /// Holds the coordination worker on a job that waits, so a wait of the UI
    /// thread is the only thing that can let the write run. The returned thread
    /// releases the worker after a delay that is longer than a frame.
    fn hold_the_worker(app: &mut crate::app::HorizonApp) -> std::thread::JoinHandle<()> {
        let (release, released) = std::sync::mpsc::channel::<()>();
        app.browser_create_host.io.write(move || {
            let _ = released.recv();
        });
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            release.send(()).expect("the worker is released");
        })
    }

    #[test]
    fn closing_a_dirty_editor_panel_writes_its_text_on_the_coordination_worker() {
        let (temp, mut app) = test_app();
        let file = temp.path().join("notes.md");
        std::fs::write(&file, "the saved text\n").expect("the file of the editor");
        let panel = dirty_editor(&mut app, &file, "the text of the person\n");

        {
            // The frame that closes the panel must hand its text to the worker.
            let _frame = horizon_core::ui_thread::FrameScope::enter();
            app.close_panel(panel);
            assert!(app.board.panel(panel).is_none(), "the panel left at once");
        }
        app.settle_browser_host_io();
        assert_eq!(
            std::fs::read_to_string(&file).expect("the file of the editor"),
            "the text of the person\n",
            "the test wrote the text of the closed panel to its file"
        );
    }

    #[test]
    fn closing_a_workspace_writes_the_text_of_its_dirty_editor() {
        let (temp, mut app) = test_app();
        let file = temp.path().join("notes.md");
        std::fs::write(&file, "the saved text\n").expect("the file of the editor");
        let panel = dirty_editor(&mut app, &file, "the text of the person\n");
        let workspace = app.board.panel(panel).expect("the panel").workspace_id;

        {
            let _frame = horizon_core::ui_thread::FrameScope::enter();
            app.close_workspace_panels(workspace);
            assert!(app.board.panel(panel).is_none(), "the panel left at once");
        }
        app.settle_browser_host_io();
        assert_eq!(
            std::fs::read_to_string(&file).expect("the file of the editor"),
            "the text of the person\n",
            "the test wrote the text of the panel that its workspace closed"
        );
    }

    #[test]
    fn a_session_switch_writes_the_text_of_a_dirty_editor_before_the_board_leaves() {
        let (temp, mut app) = test_app();
        let file = temp.path().join("notes.md");
        std::fs::write(&file, "the saved text\n").expect("the file of the editor");
        let panel = dirty_editor(&mut app, &file, "the text of the person\n");
        let target = app
            .session_store
            .create_session_from_runtime(RuntimeState::default())
            .expect("target session");

        {
            let _frame = horizon_core::ui_thread::FrameScope::enter();
            app.activate_runtime_session(&egui::Context::default(), &target);
            assert!(app.board.panel(panel).is_none(), "the old board left");
        }
        app.settle_browser_host_io();
        assert_eq!(
            std::fs::read_to_string(&file).expect("the file of the editor"),
            "the text of the person\n",
            "the switch wrote the text of the editor of the board it left"
        );
    }

    #[test]
    fn the_exit_writes_the_text_of_a_dirty_editor_before_the_window_leaves() {
        let (temp, mut app) = test_app();
        let file = temp.path().join("notes.md");
        std::fs::write(&file, "the saved text\n").expect("the file of the editor");
        let _panel = dirty_editor(&mut app, &file, "the text of the person\n");
        let releasing = hold_the_worker(&mut app);

        {
            let _frame = horizon_core::ui_thread::FrameScope::enter();
            app.begin_shutdown();
            // No frame and no settle: the cleanup itself waits for the write
            // that the closing window asked for, inside its own scope.
            app.run_exit_cleanup();
        }
        assert_eq!(
            std::fs::read_to_string(&file).expect("the file of the editor"),
            "the text of the person\n",
            "the exit wrote the text of the editor before it ended"
        );
        releasing.join().expect("the releasing thread ends");
    }

    #[test]
    fn an_exit_without_a_close_request_writes_the_text_of_a_dirty_editor() {
        let (temp, mut app) = test_app();
        let file = temp.path().join("notes.md");
        std::fs::write(&file, "the saved text\n").expect("the file of the editor");
        let _panel = dirty_editor(&mut app, &file, "the text of the person\n");
        let releasing = hold_the_worker(&mut app);

        {
            let _frame = horizon_core::ui_thread::FrameScope::enter();
            // The shutdown of the panels is the only close request of this
            // path, so it is what marks the buffer of the editor.
            app.run_exit_cleanup();
        }
        assert_eq!(
            std::fs::read_to_string(&file).expect("the file of the editor"),
            "the text of the person\n",
            "the cleanup wrote the text that no frame handed to the worker"
        );
        releasing.join().expect("the releasing thread ends");
    }

    #[test]
    fn the_close_of_the_window_queues_the_write_of_a_dirty_editor_for_its_frames() {
        let (temp, mut app) = test_app();
        let file = temp.path().join("notes.md");
        std::fs::write(&file, "the saved text\n").expect("the file of the editor");
        let _panel = dirty_editor(&mut app, &file, "the text of the person\n");
        // The worker is held before the frame that closes the window, because a
        // write of a few bytes finishes faster than the assertion that the exit
        // waits for it on a quiet disk.
        let releasing = hold_the_worker(&mut app);
        app.settle_browser_host_io();

        {
            let _frame = horizon_core::ui_thread::FrameScope::enter();
            app.begin_shutdown();
            assert!(
                app.browser_create_host.io.busy(),
                "the frames of the exit have the write of the editor to wait for"
            );
        }
        app.settle_browser_host_io();
        assert_eq!(
            std::fs::read_to_string(&file).expect("the file of the editor"),
            "the text of the person\n",
            "the write that the frames of the exit waited for reached the file"
        );
        releasing.join().expect("the releasing thread ends");
    }

    #[test]
    fn the_write_of_a_closed_panel_runs_after_the_writes_asked_for_before_it() {
        let (temp, mut app) = test_app();
        let file = temp.path().join("notes.md");
        std::fs::write(&file, "the saved text\n").expect("the file of the editor");
        let panel = dirty_editor(&mut app, &file, "the first text\n");

        // A job that waits holds the worker, so both writes of the file are
        // asked for before either one runs.
        let (release, released) = std::sync::mpsc::channel::<()>();
        app.browser_create_host.io.write(move || {
            let _ = released.recv();
        });
        {
            let _frame = horizon_core::ui_thread::FrameScope::enter();
            let editor = app
                .board
                .panel_mut(panel)
                .and_then(Panel::editor_mut)
                .expect("the editor of the panel");
            editor.request_save();
            app.queue_editor_saves();
            let editor = app
                .board
                .panel_mut(panel)
                .and_then(Panel::editor_mut)
                .expect("the editor of the panel");
            "the text of the person\n".clone_into(&mut editor.text);
            editor.dirty = true;
            app.close_panel(panel);
        }
        release.send(()).expect("the worker is released");
        app.settle_browser_host_io();
        assert_eq!(
            std::fs::read_to_string(&file).expect("the file of the editor"),
            "the text of the person\n",
            "the write of the closed panel ran after the write asked for before it"
        );
    }

    #[test]
    fn a_write_that_fails_as_its_panel_leaves_leaves_one_line() {
        let (temp, mut app) = test_app();
        let file = temp.path().join("notes.md");
        std::fs::create_dir(&file).expect("a directory where the file was");
        let panel = dirty_editor(&mut app, &file, "the text that cannot be written\n");

        {
            let _frame = horizon_core::ui_thread::FrameScope::enter();
            app.close_panel(panel);
        }
        app.settle_browser_host_io();
        assert!(closed_editor_text(&app, panel).is_none(), "the panel left");
        let line = app
            .sidebar_sticky_notice
            .clone()
            .expect("the person reads why the text of the closed panel is not in its file");
        assert!(
            line.contains("Could not save") && line.contains("notes.md"),
            "the line names the failure and the file: {line}"
        );
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
