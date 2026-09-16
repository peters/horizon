use super::{Msg, TermMode, Terminal, TerminalDimensions, WindowSize};

pub(super) fn queue_debounced_pty_resize(
    pending_pty_resize: &mut Option<std::time::Instant>,
    requested_at: std::time::Instant,
) {
    pending_pty_resize.get_or_insert(requested_at);
}

pub(super) fn should_debounce_pty_resize(pty_resized: bool, mode: TermMode) -> bool {
    pty_resized && !mode.contains(TermMode::ALT_SCREEN)
}

impl Terminal {
    pub fn resize(&mut self, rows: u16, cols: u16, cell_width: u16, cell_height: u16) {
        self.resize_with_policy(rows, cols, cell_width, cell_height, false);
    }

    pub fn resize_immediately(&mut self, rows: u16, cols: u16, cell_width: u16, cell_height: u16) {
        self.resize_with_policy(rows, cols, cell_width, cell_height, true);
    }

    fn resize_with_policy(&mut self, rows: u16, cols: u16, cell_width: u16, cell_height: u16, immediate: bool) {
        let rows = rows.max(1);
        let cols = cols.max(2);
        let cell_width = cell_width.max(1);
        let cell_height = cell_height.max(1);
        let mode = self.mode();

        if rows == self.rows && cols == self.cols && cell_width == self.cell_width && cell_height == self.cell_height {
            if immediate {
                self.send_pty_resize();
            } else {
                self.flush_pending_pty_resize();
            }
            return;
        }

        self.rows = rows;
        self.cols = cols;
        self.cell_width = cell_width;
        self.cell_height = cell_height;

        // Resize the terminal grid immediately for smooth visual feedback.
        self.term.lock().resize(TerminalDimensions::new(self.rows, self.cols));

        if immediate || !should_debounce_pty_resize(self.pty_resized, mode) {
            self.send_pty_resize();
            return;
        }

        // Keep the first deferred resize request timestamp so repeated
        // drag-resize events still flush through to the child process at a
        // bounded cadence instead of being postponed indefinitely.
        queue_debounced_pty_resize(&mut self.pending_pty_resize, std::time::Instant::now());
    }

    pub(super) fn flush_pending_pty_resize(&mut self) {
        const PTY_RESIZE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(80);

        let Some(requested_at) = self.pending_pty_resize else {
            return;
        };
        if requested_at.elapsed() < PTY_RESIZE_DEBOUNCE {
            return;
        }
        self.send_pty_resize();
    }

    fn send_pty_resize(&mut self) {
        self.pending_pty_resize = None;
        self.pty_resized = true;
        let _ = self
            .event_sender
            .send(Msg::Resize(self.window_size()))
            .map_err(|error| tracing::debug!("failed to resize terminal PTY: {error}"));
    }

    pub(super) fn window_size(&self) -> WindowSize {
        WindowSize {
            num_lines: self.rows,
            num_cols: self.cols,
            cell_width: self.cell_width,
            cell_height: self.cell_height,
        }
    }
}
