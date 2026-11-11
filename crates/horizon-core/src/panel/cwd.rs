//! The working directory of the process of a panel.
//!
//! `Panel::process_output` records that output may have changed the directory,
//! and the coordination worker answers with the directory that it read. A frame
//! never reads the directory itself: on macOS that read starts `pgrep` and
//! `lsof` (see `terminal::cwd`). The app asks for the read and keeps the answer,
//! so this module holds only the value and the rule that decides which kinds of
//! panel follow their process.

use std::path::PathBuf;

use super::{Panel, PanelKind, Terminal};

impl Panel {
    /// Whether the directory of a panel follows the directory of its process. A
    /// shell and a command follow it, because the person changes it with `cd`. An
    /// agent keeps the directory that it started in, which is the project it
    /// works on.
    pub(super) fn should_track_live_cwd(&self) -> bool {
        matches!(self.kind, PanelKind::Shell | PanelKind::Command)
    }

    /// The process whose working directory a worker reads next, when output may
    /// have changed it. A frame only asks: the read is the work of a worker.
    #[must_use]
    pub fn cwd_read(&self) -> Option<u32> {
        self.cwd_read
    }

    /// Takes the request of a read, so the worker answers it. Output that comes
    /// while that read runs records a request of its own, which a later frame
    /// takes, so a directory that changed in between is read again.
    pub fn take_cwd_read(&mut self) -> Option<u32> {
        self.cwd_read.take()
    }

    /// Records the directory that a worker read for the process `pid`, and says
    /// whether the directory that this panel keeps changed.
    ///
    /// An answer that names another process than the one that this panel runs now
    /// belongs to a process that is gone, so it is dropped.
    pub fn record_working_directory(&mut self, pid: u32, directory: PathBuf) -> bool {
        if self.content.terminal().and_then(Terminal::child_pid) != Some(pid) || !self.should_track_live_cwd() {
            return false;
        }
        if self.launch_cwd.as_ref() == Some(&directory) {
            return false;
        }
        self.launch_cwd = Some(directory);
        true
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::super::{Panel, PanelId, PanelKind, PanelOptions, WorkspaceId};

    /// A panel that runs a process of this computer, with the process id that
    /// the worker that reads its directory reports.
    fn panel_with_a_process(kind: PanelKind) -> (Panel, u32) {
        let panel = Panel::spawn(
            PanelId(1),
            WorkspaceId(1),
            PanelOptions {
                kind,
                command: Some("/bin/sh".to_owned()),
                args: vec!["-c".to_owned(), "sleep 5".to_owned()],
                ..PanelOptions::default()
            },
        )
        .expect("the panel of the test starts");
        let pid = panel
            .terminal()
            .and_then(crate::terminal::Terminal::child_pid)
            .expect("a child process");
        (panel, pid)
    }

    #[test]
    fn shell_panels_track_latest_live_cwd() {
        let (mut panel, pid) = panel_with_a_process(PanelKind::Shell);
        panel.launch_cwd = Some(PathBuf::from("/workspace"));

        assert!(panel.record_working_directory(pid, PathBuf::from("/workspace/subdir")));
        assert_eq!(panel.launch_cwd, Some(PathBuf::from("/workspace/subdir")));
    }

    #[test]
    fn agent_panels_keep_original_launch_cwd() {
        let (mut panel, pid) = panel_with_a_process(PanelKind::Pi);
        panel.launch_cwd = Some(PathBuf::from("/workspace"));

        assert!(!panel.record_working_directory(pid, PathBuf::from("/workspace/subdir")));
        assert_eq!(panel.launch_cwd, Some(PathBuf::from("/workspace")));
    }

    #[test]
    fn an_answer_for_another_process_names_no_directory_of_this_panel() {
        let (mut panel, pid) = panel_with_a_process(PanelKind::Shell);
        panel.launch_cwd = Some(PathBuf::from("/workspace"));

        assert!(!panel.record_working_directory(pid + 1, PathBuf::from("/workspace/subdir")));
        assert_eq!(panel.launch_cwd, Some(PathBuf::from("/workspace")));
    }

    #[test]
    fn a_request_of_a_read_goes_when_the_worker_takes_it() {
        let (mut panel, pid) = panel_with_a_process(PanelKind::Shell);
        panel.cwd_read = Some(pid);

        assert_eq!(panel.take_cwd_read(), Some(pid));
        assert_eq!(panel.cwd_read(), None);
    }
}
