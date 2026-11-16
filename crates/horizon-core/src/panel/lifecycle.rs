use crate::editor::{MarkdownEditor, PanelContent};
use crate::error::{Error, Result};
use crate::ssh::SshConnectionStatus;
use crate::terminal::{Terminal, TerminalSpawnOptions};

use super::spawn::{
    PanelStart, PreparedLaunch, current_unix_millis, kitty_keyboard_for_kind, replay_bytes_with_notice,
    scrollback_limit_for_kind,
};
use super::{DEFAULT_CELL_HEIGHT, DEFAULT_CELL_WIDTH, Panel, PanelKind};

impl Panel {
    /// The screen that a restart of this panel keeps, as the output that draws
    /// it again in the new terminal.
    ///
    /// A terminal that attaches a tmux session on a machine keeps its screen:
    /// the session still holds the work, so the person reads the last screen of
    /// it until the new connection draws. A panel whose program ended keeps its
    /// screen above the new program, which starts under the text that the person
    /// read before. Every other panel starts with the empty screen that its own
    /// new output fills.
    pub(super) fn restart_replay_bytes(&self) -> Vec<u8> {
        if self.program_end.is_some() {
            let mut replay = self.terminal().map(Terminal::screen_replay_bytes).unwrap_or_default();
            if !replay.is_empty() {
                // The cursor moves under the text that the old program left, so
                // the new program writes below it instead of over it.
                replay.extend_from_slice(b"\r\n");
            }
            return replay;
        }
        if !self.attaches_a_tmux_session() {
            return Vec::new();
        }
        self.terminal().map(Terminal::screen_replay_bytes).unwrap_or_default()
    }

    /// Restart panel content while keeping the same identity and layout.
    /// Terminal agent panels resume their existing session. Browser panels
    /// only schedule an asynchronous relaunch; `Ok(())` confirms that the
    /// request was accepted, while launch failures arrive through Browser
    /// status/events.
    ///
    /// # Errors
    ///
    /// Returns an error if a terminal cannot be spawned, a file-backed editor
    /// cannot be reopened, or a retired remote task is requested. Continuation
    /// also refuses stale authorization, unverifiable saved history or launch
    /// ownership, and incomplete shutdown of the previous process.
    pub fn restart(&mut self) -> Result<()> {
        let Some(request) = self.restart_request()? else {
            return Ok(());
        };
        let mut preflight = super::spawn::prepare_restart(request)?;
        let wanted = preflight.work_brief_wanted();
        if let Some(ready) = preflight.take_ready() {
            return self.apply_restart(ready);
        }
        let Some(pending) = preflight.into_pending() else {
            return Ok(());
        };
        super::work_resume::shutdown_for_restart(
            self.terminal_mut()
                .ok_or_else(|| Error::State("No terminal to restart".into()))?,
            wanted,
        )?;
        self.apply_restart(super::spawn::finish_restart(pending)?)
    }

    /// Restarts the content of this panel with the answer that a worker read.
    ///
    /// # Errors
    ///
    /// Returns the refusal of the pre-flight, or the error of the process start.
    pub fn apply_restart(&mut self, start: PanelStart) -> Result<()> {
        if let PanelContent::Browser(browser) = &mut self.content {
            browser.relaunch();
            return Ok(());
        }
        if let Some(text) = start.editor_text {
            let text = text.map_err(Error::Editor)?;
            if let PanelContent::Editor(editor) = &mut self.content
                && let Some(path) = editor.file_path.clone()
                && path.exists()
            {
                *editor = MarkdownEditor::from_file(path, text);
            }
            return Ok(());
        }
        if let Some(failure) = &start.working_directory {
            self.show_working_directory_failure(failure)?;
            return Ok(());
        }
        let Some(terminal) = self.terminal() else {
            return Err(Error::State("No terminal to restart".into()));
        };
        let (rows, cols) = (terminal.rows(), terminal.cols());
        let Some(launch) = start.launch else {
            return Ok(());
        };
        // This restart starts the program of the panel here, so it has no link to a
        // holder: the restart of the app runs on the worker, which reads the setting.
        let PreparedLaunch {
            session_binding,
            program,
            args,
            env,
            work,
            named_secrets_late,
            notice,
            ..
        } = launch;
        let mut replay_bytes = replay_bytes_with_notice(start.replay_bytes, named_secrets_late);
        if let Some(notice) = notice {
            replay_bytes.extend_from_slice(notice.as_bytes());
        }
        self.content = PanelContent::Terminal(Terminal::spawn(TerminalSpawnOptions {
            program,
            args,
            cwd: self.launch_cwd.clone(),
            rows,
            cols,
            cell_width: DEFAULT_CELL_WIDTH,
            cell_height: DEFAULT_CELL_HEIGHT,
            scrollback_limit: scrollback_limit_for_kind(self.kind),
            window_id: self.id.0,
            replay_bytes,
            env,
            kitty_keyboard: kitty_keyboard_for_kind(self.kind),
        })?);
        self.session_binding = session_binding;
        if let Some(terminal) = self.terminal_mut() {
            terminal.work_owner = work.owner;
            terminal.work_continuation = work.state;
        }
        self.launched_at_millis = current_unix_millis();
        self.clear_program_end();
        self.ssh_connected_at_millis = None;
        self.ssh_exit_recorded = false;
        self.ssh_end = None;
        self.ssh_reconnect_attempt = 0;
        self.ssh_status = if self.kind == PanelKind::Ssh {
            Some(SshConnectionStatus::Connecting)
        } else {
            None
        };
        tracing::info!("restarted panel '{}' (id={})", self.title, self.id.0);
        Ok(())
    }
}
