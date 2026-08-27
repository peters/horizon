use alacritty_terminal::term::cell::{Cell, Flags, Hyperlink};

use super::{
    Column, Dimensions, PathBuf, Point, RenderableContent, Scroll, Term, TermDamage, Terminal, TerminalEventProxy,
    current_cwd_for_pid,
    file_citation::{file_citation_target_at_column, wrapped_line_contains_file_citation},
    find_file_path_at_column, find_url_at_column, viewport_to_point,
};

impl Terminal {
    #[must_use]
    pub fn scrollback(&self) -> usize {
        self.term.lock().grid().display_offset()
    }

    pub fn set_scrollback(&mut self, scrollback: usize) {
        let current = self.scrollback();
        if current == scrollback {
            return;
        }

        let current = isize::try_from(current).unwrap_or(isize::MAX);
        let target = isize::try_from(scrollback).unwrap_or(isize::MAX);
        let delta = target.saturating_sub(current);
        let delta = delta.clamp(i32::MIN as isize, i32::MAX as isize);
        #[allow(clippy::cast_possible_truncation)]
        let delta = delta as i32;

        self.term.lock().scroll_display(Scroll::Delta(delta));
    }

    pub fn scroll_scrollback_by(&mut self, delta: i32) {
        if delta == 0 {
            return;
        }

        let current = self.scrollback();
        let target = if delta.is_positive() {
            current.saturating_add(usize::try_from(delta).unwrap_or(usize::MAX))
        } else {
            current.saturating_sub(usize::try_from(delta.unsigned_abs()).unwrap_or(usize::MAX))
        };
        self.set_scrollback(target);
    }

    /// Extract the last few non-empty lines visible on screen as a single
    /// string, for pattern matching (e.g. detecting agent prompts).
    #[must_use]
    pub fn last_lines_text(&self, max_lines: usize) -> String {
        let term = self.term.lock();
        let content = term.renderable_content();
        let cols = usize::from(self.cols);
        let rows = usize::from(self.rows);
        let mut lines: Vec<String> = Vec::with_capacity(max_lines);
        let mut current_line = String::with_capacity(cols);
        let mut current_line_columns = 0;
        let mut current_row: Option<usize> = None;

        for indexed in content.display_iter {
            let Ok(row) = usize::try_from(indexed.point.line.0) else {
                continue;
            };
            if row >= rows {
                continue;
            }
            if current_row != Some(row) {
                if !current_line.is_empty() {
                    lines.push(std::mem::take(&mut current_line));
                }
                current_row = Some(row);
                current_line.clear();
                current_line_columns = 0;
            }
            append_cell_text(
                &mut current_line,
                &mut current_line_columns,
                indexed.point.column.0,
                indexed.cell,
            );
        }
        if !current_line.is_empty() {
            lines.push(current_line);
        }
        let start = lines.len().saturating_sub(max_lines);
        lines[start..].join("\n")
    }

    /// Extract the text of the bottom `max_rows` visible rows of the screen
    /// (empty rows omitted), for detecting status lines that agent TUIs pin
    /// near the bottom of the screen.
    #[must_use]
    pub fn bottom_lines_text(&self, max_rows: usize) -> Vec<String> {
        let term = self.term.lock();
        bottom_row_texts(&term, usize::from(self.rows), max_rows)
    }

    /// Extract all text from the terminal grid including scrollback history.
    ///
    /// Returns `(lines, grid_total)` where `grid_total` is the total number
    /// of grid lines (scrollback + screen, capped at `max_lines`) *before*
    /// trailing-empty-line trimming.  Callers can use `grid_total` together
    /// with a line index to compute a scrollback offset.
    ///
    /// Lines are ordered oldest (top of scrollback) to newest (bottom of
    /// screen). Each line is trimmed of trailing whitespace. The extraction
    /// locks the terminal mutex once and copies text in a single pass.
    #[must_use]
    pub fn full_text_lines(&self, max_lines: usize) -> (Vec<String>, usize) {
        let term = self.term.lock();
        let grid = term.grid();
        let cols = grid.columns();
        let total = grid.total_lines().min(max_lines);
        let screen_lines = grid.screen_lines();

        let mut lines: Vec<String> = Vec::with_capacity(total);

        for raw_line_idx in 0..total {
            // Grid line indexing: 0 is top of screen, negative indices
            // are scrollback history. We iterate from oldest to newest.
            let history_offset = total.saturating_sub(screen_lines);
            let line_idx = if raw_line_idx < history_offset {
                // Scrollback region: negative line indices.
                // Line -(history_offset - raw_line_idx) in grid coords.
                #[allow(clippy::cast_possible_wrap)]
                let idx = -(i32::try_from(history_offset - raw_line_idx).unwrap_or(i32::MAX));
                alacritty_terminal::index::Line(idx)
            } else {
                // Screen region: 0..screen_lines.
                #[allow(clippy::cast_possible_wrap)]
                let idx = i32::try_from(raw_line_idx - history_offset).unwrap_or(i32::MAX);
                alacritty_terminal::index::Line(idx)
            };

            let mut line = String::with_capacity(cols);
            let mut occupied_columns = 0;
            for col in 0..cols {
                let cell = &grid[line_idx][Column(col)];
                append_cell_text(&mut line, &mut occupied_columns, col, cell);
            }
            let trimmed_len = line.trim_end().len();
            line.truncate(trimmed_len);
            lines.push(line);
        }

        // Drop empty trailing lines.
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }

        (lines, total)
    }

    #[must_use]
    pub fn scrollback_limit(&self) -> usize {
        self.scrollback_limit
    }

    #[must_use]
    pub fn history_size(&self) -> usize {
        let term = self.term.lock();
        let grid = term.grid();
        grid.total_lines().saturating_sub(grid.screen_lines())
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
    pub fn current_cwd(&self) -> Option<PathBuf> {
        current_cwd_for_pid(self.child_pid?)
    }

    #[must_use]
    pub fn child_exited(&self) -> bool {
        self.child_exited
    }

    /// Returns the exit status of the child process if it has exited *and* a
    /// status was reported. `None` while the child is still running, or after
    /// an `Event::Exit` that didn't carry a status (e.g. internal teardown).
    #[must_use]
    pub fn child_exit_status(&self) -> Option<std::process::ExitStatus> {
        self.child_exit_status
    }

    pub fn with_renderable_content<R>(&self, render: impl FnOnce(RenderableContent<'_>) -> R) -> R {
        let term = self.term.lock();
        render(term.renderable_content())
    }

    pub fn with_damage<R>(&self, update: impl FnOnce(TermDamage<'_>) -> R) -> R {
        let mut term = self.term.lock();
        update(term.damage())
    }

    pub fn reset_damage(&self) {
        self.term.lock().reset_damage();
    }

    /// Return a clickable target at the given viewport-relative row and
    /// column. Semantic links take priority over in-band URL or path text.
    #[must_use]
    pub fn clickable_at_point(&self, row: usize, col: usize) -> Option<String> {
        let term = self.term.lock();
        let cols = usize::from(self.cols);
        if let Some(uri) = hyperlink_uri_at_viewport_point(&term, cols, row, col) {
            return Some(uri);
        }
        let (line_chars, logical_col) = wrapped_line_chars_at_viewport_point(&term, cols, row, col)?;

        file_citation_target_at_column(&line_chars, logical_col)
            .or_else(|| find_url_at_column(&line_chars, logical_col))
            .or_else(|| find_file_path_at_column(&line_chars, logical_col))
    }

    /// Return a target that is safe to open with an unmodified primary click.
    #[must_use]
    pub fn plain_clickable_at_point(&self, row: usize, col: usize) -> Option<String> {
        let term = self.term.lock();
        let cols = usize::from(self.cols);
        if let Some(uri) = hyperlink_uri_at_viewport_point(&term, cols, row, col) {
            return Some(uri);
        }
        if !wrapped_line_contains_file_citation(&term, cols, row, col) {
            return None;
        }
        let (line_chars, logical_col) = wrapped_line_chars_at_viewport_point(&term, cols, row, col)?;
        file_citation_target_at_column(&line_chars, logical_col)
    }

    /// Return the OSC 8 hyperlink URI at the given viewport-relative cell.
    #[must_use]
    pub fn hyperlink_at_point(&self, row: usize, col: usize) -> Option<String> {
        let term = self.term.lock();
        hyperlink_uri_at_viewport_point(&term, usize::from(self.cols), row, col)
    }

    /// Return whether the given viewport-relative cell has an OSC 8 hyperlink.
    #[must_use]
    pub fn has_hyperlink_at_point(&self, row: usize, col: usize) -> bool {
        let term = self.term.lock();
        hyperlink_at_viewport_point(&term, usize::from(self.cols), row, col).is_some()
    }
}

fn hyperlink_uri_at_viewport_point<T>(term: &Term<T>, cols: usize, row: usize, col: usize) -> Option<String> {
    hyperlink_at_viewport_point(term, cols, row, col).map(|hyperlink| hyperlink.uri().to_owned())
}

fn hyperlink_at_viewport_point<T>(term: &Term<T>, cols: usize, row: usize, col: usize) -> Option<Hyperlink> {
    if col >= cols {
        return None;
    }
    let grid = term.grid();
    if row >= grid.screen_lines() {
        return None;
    }
    let point = viewport_to_point(grid.display_offset(), Point::new(row, Column(col)));
    let cell = &grid[point.line][Column(col)];
    if let Some(hyperlink) = nonempty_hyperlink(cell) {
        return Some(hyperlink);
    }
    if cell.flags.contains(Flags::WIDE_CHAR_SPACER) && col > 0 {
        return nonempty_hyperlink(&grid[point.line][Column(col - 1)]);
    }
    None
}

fn nonempty_hyperlink(cell: &Cell) -> Option<Hyperlink> {
    let hyperlink = cell.hyperlink()?;
    (!hyperlink.uri().is_empty()).then_some(hyperlink)
}

fn wrapped_line_chars_at_viewport_point<T>(
    term: &Term<T>,
    cols: usize,
    row: usize,
    col: usize,
) -> Option<(Vec<char>, usize)> {
    if col >= cols {
        return None;
    }

    let grid = term.grid();
    if row >= grid.screen_lines() {
        return None;
    }

    let point = viewport_to_point(grid.display_offset(), Point::new(row, Column(col)));
    let start = term.line_search_left(point);
    let end = term.line_search_right(point);
    let mut line_chars = Vec::with_capacity(cols);
    let mut logical_col = 0;
    let mut line = start.line;

    loop {
        if line == point.line {
            logical_col = line_chars.len() + col;
        }

        for column in 0..cols {
            line_chars.push(grid[line][Column(column)].c);
        }

        if line == end.line {
            break;
        }

        line += 1;
    }

    Some((line_chars, logical_col))
}

/// Text of the non-empty rows within the bottom `max_rows` rows of a visible
/// screen, in top-to-bottom order.
#[must_use]
fn bottom_row_texts(term: &Term<TerminalEventProxy>, rows: usize, max_rows: usize) -> Vec<String> {
    let content = term.renderable_content();
    let start_row = rows.saturating_sub(max_rows);
    let mut lines: Vec<String> = Vec::new();
    let mut current_line = String::new();
    let mut current_line_columns = 0;
    let mut current_row: Option<usize> = None;

    for indexed in content.display_iter {
        let Ok(row) = usize::try_from(indexed.point.line.0) else {
            continue;
        };
        if row < start_row || row >= rows {
            continue;
        }
        if current_row != Some(row) {
            if current_row.is_some() && !current_line.is_empty() {
                lines.push(std::mem::take(&mut current_line));
            }
            current_row = Some(row);
            current_line.clear();
            current_line_columns = 0;
        }
        append_cell_text(
            &mut current_line,
            &mut current_line_columns,
            indexed.point.column.0,
            indexed.cell,
        );
    }
    if current_row.is_some() && !current_line.is_empty() {
        lines.push(current_line);
    }

    lines
}

fn append_cell_text(line: &mut String, occupied_columns: &mut usize, target_column: usize, cell: &Cell) {
    if cell
        .flags
        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        || (cell.c == ' ' && cell.zerowidth().is_none())
    {
        return;
    }

    // Terminal columns are not the same as UTF-8 bytes, so track occupied
    // columns separately to preserve spacing after multibyte and wide glyphs.
    while *occupied_columns < target_column {
        line.push(' ');
        *occupied_columns += 1;
    }

    line.push(cell.c);
    if let Some(chars) = cell.zerowidth() {
        for ch in chars {
            line.push(*ch);
        }
    }

    *occupied_columns = target_column + if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use alacritty_terminal::term::cell::{Cell, Flags};
    use alacritty_terminal::term::{self, Term};
    use alacritty_terminal::vte::ansi;

    use super::{
        append_cell_text, bottom_row_texts, hyperlink_uri_at_viewport_point, wrapped_line_chars_at_viewport_point,
    };
    use crate::terminal::{TerminalDimensions, TerminalEventProxy, find_url_at_column};

    fn reconstruct_line(cells: &[(usize, Cell)]) -> String {
        let mut line = String::new();
        let mut occupied_columns = 0;

        for (column, cell) in cells {
            append_cell_text(&mut line, &mut occupied_columns, *column, cell);
        }

        line
    }

    #[test]
    fn multibyte_glyphs_preserve_following_padding() {
        let accent_cell = Cell {
            c: 'é',
            ..Cell::default()
        };
        let x_cell = Cell {
            c: 'x',
            ..Cell::default()
        };

        let line = reconstruct_line(&[(0, accent_cell), (2, x_cell)]);

        assert_eq!(line, "é x");
    }

    #[test]
    fn combining_marks_stay_attached_to_base_cell() {
        let mut base_cell = Cell {
            c: 'e',
            ..Cell::default()
        };
        base_cell.push_zerowidth('\u{0301}');
        let x_cell = Cell {
            c: 'x',
            ..Cell::default()
        };

        let line = reconstruct_line(&[(0, base_cell), (1, x_cell)]);

        assert_eq!(line, "e\u{0301}x");
    }

    #[test]
    fn variation_selectors_stay_attached_to_base_cell() {
        let mut base_cell = Cell {
            c: '✈',
            ..Cell::default()
        };
        base_cell.push_zerowidth('\u{fe0f}');
        let x_cell = Cell {
            c: 'x',
            ..Cell::default()
        };

        let line = reconstruct_line(&[(0, base_cell), (1, x_cell)]);

        assert_eq!(line, "✈\u{fe0f}x");
    }

    #[test]
    fn wide_glyphs_consume_two_terminal_columns() {
        let wide_cell = Cell {
            c: '你',
            flags: Flags::WIDE_CHAR,
            ..Cell::default()
        };
        let x_cell = Cell {
            c: 'x',
            ..Cell::default()
        };

        let line = reconstruct_line(&[(0, wide_cell), (2, x_cell)]);

        assert_eq!(line, "你x");
    }

    fn test_term(rows: u16, cols: u16) -> Term<TerminalEventProxy> {
        let (event_tx, _event_rx) = mpsc::channel();
        let dimensions = TerminalDimensions::new(rows, cols);
        let config = term::Config {
            scrolling_history: 256,
            kitty_keyboard: true,
            ..term::Config::default()
        };

        Term::new(config, &dimensions, TerminalEventProxy { event_tx })
    }

    #[test]
    fn bottom_row_texts_only_includes_the_bottom_window() {
        let mut term = test_term(6, 20);
        let mut parser = ansi::Processor::<ansi::StdSyncHandler>::default();
        parser.advance(
            &mut term,
            b"top-one\r\ntop-two\r\nmid-one\r\nmid-two\r\nmid-three\r\nworking-line",
        );

        let lines = bottom_row_texts(&term, 6, 3);

        assert_eq!(lines, vec!["mid-two", "mid-three", "working-line"]);
    }

    #[test]
    fn bottom_row_texts_omits_empty_rows_and_accepts_wide_window() {
        let mut term = test_term(8, 20);
        let mut parser = ansi::Processor::<ansi::StdSyncHandler>::default();
        // Row 0 carries text, rows 1-5 are empty, row 6 has the status line.
        parser.advance(&mut term, b"alpha\r\n\r\n\r\n\r\n\r\n\r\n\xe2\xa0\x8b Working...\n");

        let lines = bottom_row_texts(&term, 8, 4);

        assert_eq!(lines, vec!["\u{280b} Working..."]);

        let all = bottom_row_texts(&term, 8, 16);
        assert_eq!(all, vec!["alpha", "\u{280b} Working..."]);
    }

    #[test]
    fn wrapped_url_detection_includes_continuation_rows() {
        let url = "https://example.com/very/long/path";
        let mut term = test_term(4, 12);
        let mut parser = ansi::Processor::<ansi::StdSyncHandler>::default();
        parser.advance(&mut term, url.as_bytes());

        let (line_chars, logical_col) =
            wrapped_line_chars_at_viewport_point(&term, 12, 2, 4).expect("wrapped line should be present");

        assert_eq!(find_url_at_column(&line_chars, logical_col), Some(url.to_string()));
    }

    #[test]
    fn osc8_hyperlink_is_detected_at_labeled_cells() {
        let mut term = test_term(4, 40);
        let mut parser = ansi::Processor::<ansi::StdSyncHandler>::default();
        parser.advance(
            &mut term,
            b"Read \x1b]8;;https://x.ai/terms\x07Terms\x1b]8;;\x07 and more",
        );

        assert_eq!(
            hyperlink_uri_at_viewport_point(&term, 40, 0, 5),
            Some("https://x.ai/terms".to_string())
        );
        assert_eq!(
            hyperlink_uri_at_viewport_point(&term, 40, 0, 9),
            Some("https://x.ai/terms".to_string())
        );
        assert_eq!(hyperlink_uri_at_viewport_point(&term, 40, 0, 0), None);
        assert_eq!(hyperlink_uri_at_viewport_point(&term, 40, 0, 11), None);
    }

    #[test]
    fn osc8_hyperlink_takes_priority_over_visible_url_text() {
        let mut term = test_term(4, 40);
        let mut parser = ansi::Processor::<ansi::StdSyncHandler>::default();
        parser.advance(
            &mut term,
            b"\x1b]8;;https://x.ai/terms\x07https://example.com\x1b]8;;\x07",
        );

        assert_eq!(
            hyperlink_uri_at_viewport_point(&term, 40, 0, 0),
            Some("https://x.ai/terms".to_string())
        );
        assert_eq!(
            hyperlink_uri_at_viewport_point(&term, 40, 0, 8),
            Some("https://x.ai/terms".to_string())
        );
    }

    #[test]
    fn wrapped_file_citation_is_plain_clickable() {
        let token = r#":codex-file-citation{path="/tmp/guides/Linux install.pdf" purpose="source"}"#;
        let mut term = test_term(6, 24);
        let mut parser = ansi::Processor::<ansi::StdSyncHandler>::default();
        parser.advance(&mut term, format!("Read {token}").as_bytes());
        let (line_chars, logical_col) =
            wrapped_line_chars_at_viewport_point(&term, 24, 2, 5).expect("wrapped citation");

        assert_eq!(
            crate::terminal::file_citation::file_citation_target_at_column(&line_chars, logical_col),
            Some("/tmp/guides/Linux install.pdf".to_owned())
        );
    }
}
