use std::time::Duration;

use crate::editor::{MarkdownEditor, PanelContent};
use crate::error::Result;
use crate::runtime_state::claude_session_transcript_exists;
use crate::ssh::SshConnectionStatus;
use crate::terminal::{Terminal, TerminalSpawnOptions};
use crate::usage_dashboard::UsageDashboard;

use super::spawn::{
    AgentLaunchContext, agent_env, current_unix_millis, kitty_keyboard_for_kind, resolve_launch_command,
    scrollback_limit_for_kind,
};
use super::{DEFAULT_CELL_HEIGHT, DEFAULT_CELL_WIDTH, Panel, PanelKind};

impl Panel {
    /// Restart panel content while keeping the same identity and layout.
    /// Terminal agent panels resume their existing session. Browser panels
    /// only schedule an asynchronous relaunch; `Ok(())` confirms that the
    /// request was accepted, while launch failures arrive through Browser
    /// status/events.
    ///
    /// # Errors
    ///
    /// Returns an error if a terminal cannot be spawned or a file-backed
    /// editor cannot be reopened.
    pub fn restart(&mut self) -> Result<()> {
        if let PanelContent::GitChanges(_) = &self.content {
            return Ok(());
        }

        if let PanelContent::Browser(browser) = &mut self.content {
            browser.relaunch();
            return Ok(());
        }

        if let PanelContent::Usage(_) = &self.content {
            self.content = PanelContent::Usage(UsageDashboard::new());
            return Ok(());
        }

        let Some(terminal) = self.content.terminal_mut() else {
            // Editor panels don't restart — just reload from disk if file-backed.
            if let Some(editor) = self.content.editor_mut()
                && let Some(path) = editor.file_path.clone()
                && path.exists()
            {
                *editor = MarkdownEditor::open(path)?;
            }
            return Ok(());
        };

        let rows = terminal.rows();
        let cols = terminal.cols();

        // Graceful shutdown of the old terminal.
        let _ = terminal.shutdown_with_timeout(Duration::from_secs(2));

        // A pre-assigned Claude binding may not have a transcript yet (panel
        // never received a message); resuming it would fail, so relaunch
        // fresh under the same session id instead.
        let should_resume = self.kind.supports_session_binding()
            && self.session_binding.as_ref().is_some_and(|binding| {
                self.kind != PanelKind::Claude || claude_session_transcript_exists(&binding.session_id)
            });
        let (program, launch_args) = resolve_launch_command(
            self.launch_command.clone(),
            self.launch_args.clone(),
            self.ssh_connection.clone(),
            self.kind,
            AgentLaunchContext {
                resume: &self.resume,
                session_binding: self.session_binding.as_ref(),
                should_resume_binding: should_resume,
                is_restore: true,
            },
        );

        if self.kind.is_agent() {
            tracing::info!(
                panel_id = self.id.0,
                kind = ?self.kind,
                resume = ?self.resume,
                session_id = self.session_binding.as_ref().map(|b| b.session_id.as_str()),
                should_resume,
                cwd = self.launch_cwd.as_ref().map(|p| p.display().to_string()).as_deref(),
                cmd = %format!("{program} {}", launch_args.join(" ")),
                "restarting agent panel"
            );
        }

        let env = agent_env(self.kind, &self.local_id);
        self.content = PanelContent::Terminal(Terminal::spawn(TerminalSpawnOptions {
            program,
            args: launch_args,
            cwd: self.launch_cwd.clone(),
            rows,
            cols,
            cell_width: DEFAULT_CELL_WIDTH,
            cell_height: DEFAULT_CELL_HEIGHT,
            scrollback_limit: scrollback_limit_for_kind(self.kind),
            window_id: self.id.0,
            replay_bytes: Vec::new(),
            env,
            kitty_keyboard: kitty_keyboard_for_kind(self.kind),
        })?);

        self.launched_at_millis = current_unix_millis();
        self.ssh_status = if self.kind == PanelKind::Ssh {
            Some(SshConnectionStatus::Connecting)
        } else {
            None
        };
        tracing::info!("restarted panel '{}' (id={})", self.title, self.id.0);
        Ok(())
    }
}
