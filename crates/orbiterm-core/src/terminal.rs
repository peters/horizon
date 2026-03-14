const DEFAULT_SCROLLBACK_LIMIT: usize = 8_000;

pub struct Terminal {
    parser: vt100::Parser,
    cols: u16,
    rows: u16,
    scrollback_limit: usize,
}

impl Terminal {
    #[must_use]
    pub fn new(rows: u16, cols: u16) -> Self {
        Self::with_scrollback(rows, cols, DEFAULT_SCROLLBACK_LIMIT)
    }

    #[must_use]
    pub fn with_scrollback(rows: u16, cols: u16, scrollback_limit: usize) -> Self {
        let scrollback_limit = scrollback_limit.max(1);
        Self {
            parser: vt100::Parser::new(rows, cols, scrollback_limit),
            cols,
            rows,
            scrollback_limit,
        }
    }

    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    #[must_use]
    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        if rows != self.rows || cols != self.cols {
            self.rows = rows;
            self.cols = cols;
            self.parser.set_size(rows, cols);
        }
    }

    #[must_use]
    pub fn scrollback(&self) -> usize {
        self.parser.screen().scrollback()
    }

    pub fn scroll_scrollback_by(&mut self, delta: i32) {
        self.parser
            .set_scrollback(next_scrollback_offset(self.scrollback(), delta));
    }

    #[must_use]
    pub fn scrollback_limit(&self) -> usize {
        self.scrollback_limit
    }

    #[must_use]
    pub fn cols(&self) -> u16 {
        self.cols
    }

    #[must_use]
    pub fn rows(&self) -> u16 {
        self.rows
    }

    #[must_use]
    pub fn title(&self) -> &str {
        self.parser.screen().title()
    }
}

fn next_scrollback_offset(current: usize, delta: i32) -> usize {
    if delta >= 0 {
        current.saturating_add(usize::try_from(delta).unwrap_or(usize::MAX))
    } else {
        current.saturating_sub(usize::try_from(delta.unsigned_abs()).unwrap_or(usize::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::{Terminal, next_scrollback_offset};

    #[test]
    fn scrollback_offset_moves_in_both_directions() {
        assert_eq!(next_scrollback_offset(4, 3), 7);
        assert_eq!(next_scrollback_offset(4, -2), 2);
        assert_eq!(next_scrollback_offset(1, -5), 0);
    }

    #[test]
    fn terminal_uses_requested_scrollback_limit() {
        let terminal = Terminal::with_scrollback(24, 80, 12_345);

        assert_eq!(terminal.scrollback_limit(), 12_345);
    }
}
