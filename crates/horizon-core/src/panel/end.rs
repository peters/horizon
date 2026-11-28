//! The end of the program of one panel, for a caller that has no board.
//!
//! A board ends the program of every panel that it closes or shuts down. A
//! caller that holds a panel of its own, such as the fixture of a test, has no
//! board to do that, and a dropped terminal only detaches: the recorder of the
//! session (`script`) takes the hangup of the closed terminal and waits for the
//! program under it, so both outlive the panel.

use super::{Panel, termination};
use crate::terminal::Terminal;

impl Panel {
    /// Ends the program of this panel and the processes that it started, with
    /// the two bounded steps that the close of a panel sends.
    ///
    /// This waits up to the two bounds of a close, so a caller on the UI thread of
    /// the product does not call it: the close of a panel hands the same steps to
    /// a watcher thread.
    pub fn end_program(&mut self) {
        let child = self.terminal().and_then(Terminal::hanging_child);
        self.request_shutdown();
        let Some(child) = child else {
            return;
        };
        termination::end_now(&child, &self.title);
    }
}
