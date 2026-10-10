use alacritty_terminal::term::cell::{Cell, Flags, Hyperlink};

use super::logical_line::{RowJoin, logical_line_at_viewport_point};
use super::{
    Column, Dimensions, PathBuf, Point, RenderableContent, Scroll, Term, Terminal, TerminalEventProxy,
    current_cwd_for_pid, find_file_path_at_column, find_url_at_column, viewport_to_point,
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
        last_lines_from(term.renderable_content(), self.cols, self.rows, max_lines)
    }

    /// As [`Self::last_lines_text`], or `None` when the grid lock is busy.
    ///
    /// The PTY reader holds the fair-mutex lease across its read. A waiter that
    /// takes that lease can stall until the read returns, so a test that only
    /// needs a snapshot uses the unfair try-lock and retries instead.
    #[must_use]
    pub fn try_last_lines_text(&self, max_lines: usize) -> Option<String> {
        let term = self.term.try_lock_unfair()?;
        Some(last_lines_from(
            term.renderable_content(),
            self.cols,
            self.rows,
            max_lines,
        ))
    }

    /// As [`Self::with_renderable_content`], or `None` when the grid lock is busy.
    pub fn try_with_renderable_content<R>(&self, render: impl FnOnce(RenderableContent<'_>) -> R) -> Option<R> {
        let term = self.term.try_lock_unfair()?;
        Some(render(term.renderable_content()))
    }

    /// The text of each row in the viewport, top to bottom, as the user sees it:
    /// scrolled back when the viewport is, with empty rows kept and trailing
    /// spaces and trailing empty rows removed.
    #[must_use]
    pub fn viewport_text(&self) -> Vec<String> {
        let term = self.term.lock();
        let content = term.renderable_content();
        let rows = usize::from(self.rows);
        let offset = i32::try_from(content.display_offset).unwrap_or(i32::MAX);
        let mut lines = vec![String::new(); rows];
        let mut columns = vec![0; rows];
        for indexed in content.display_iter {
            let Ok(row) = usize::try_from(indexed.point.line.0.saturating_add(offset)) else {
                continue;
            };
            if row < rows {
                append_cell_text(&mut lines[row], &mut columns[row], indexed.point.column.0, indexed.cell);
            }
        }
        for line in &mut lines {
            line.truncate(line.trim_end().len());
        }
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines
    }

    /// Extract the text of the bottom `max_rows` visible rows of the screen
    /// (empty rows omitted), for detecting status lines that agent TUIs pin
    /// near the bottom of the screen.
    #[must_use]
    pub fn bottom_lines_text(&self, max_rows: usize) -> Vec<String> {
        let term = self.term.lock();
        bottom_row_texts(&term, usize::from(self.rows), max_rows)
    }

    /// The rows of the screen, top to bottom with blank rows kept: the text of each
    /// row and the number of columns that its text occupies. Concealed text is left out.
    #[must_use]
    pub fn screen_rows(&self) -> Vec<(String, usize)> {
        let term = self.term.lock();
        screen_row_texts(&term, usize::from(self.rows), usize::from(self.rows))
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

    pub fn reset_damage(&self) {
        self.term.lock().reset_damage();
    }

    /// Return a clickable target at the given viewport-relative row and
    /// column. OSC 8 hyperlinks take priority over in-band URL or path text.
    /// URL text may continue across rows a program hard-wrapped; paths follow
    /// only the terminal's own soft wraps.
    #[must_use]
    pub fn clickable_at_point(&self, row: usize, col: usize) -> Option<String> {
        let term = self.term.lock();
        let cols = usize::from(self.cols);
        if let Some(uri) = hyperlink_uri_at_viewport_point(&term, cols, row, col) {
            return Some(uri);
        }
        // Hard-wrap joints only ever drop blank cells, so a click they leave
        // without a position has no path under it either.
        let line = logical_line_at_viewport_point(&term, cols, row, col, RowJoin::UrlHardWraps)?;
        if let Some(url) = find_url_at_column(&line.chars, line.column) {
            return Some(url);
        }
        let line = if line.joins_hard_wraps {
            logical_line_at_viewport_point(&term, cols, row, col, RowJoin::SoftWraps)?
        } else {
            line
        };

        find_file_path_at_column(&line.chars, line.column)
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

fn last_lines_from(content: RenderableContent<'_>, cols: u16, rows: u16, max_lines: usize) -> String {
    let cols = usize::from(cols);
    let rows = usize::from(rows);
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

/// Text of the non-empty rows within the bottom `max_rows` rows of a visible
/// screen, in top-to-bottom order.
#[must_use]
fn bottom_row_texts(term: &Term<TerminalEventProxy>, rows: usize, max_rows: usize) -> Vec<String> {
    screen_row_texts(term, rows, max_rows)
        .into_iter()
        .map(|(text, _)| text)
        .filter(|text| !text.is_empty())
        .collect()
}

/// As [`Terminal::screen_rows`], for the bottom `max_rows` rows. The rows come from
/// the live screen, also while the view is scrolled into the history.
fn screen_row_texts(term: &Term<TerminalEventProxy>, rows: usize, max_rows: usize) -> Vec<(String, usize)> {
    let grid = term.grid();
    (rows.saturating_sub(max_rows)..rows.min(grid.screen_lines()))
        .map(|row| {
            let line = &grid[alacritty_terminal::index::Line(i32::try_from(row).unwrap_or(i32::MAX))];
            let (mut text, mut columns) = (String::new(), 0);
            for column in 0..grid.columns() {
                // Concealed text stays hidden: the terminal does not draw it either.
                let cell = &line[Column(column)];
                if !cell.flags.contains(Flags::HIDDEN) {
                    append_cell_text(&mut text, &mut columns, column, cell);
                }
            }
            (text, columns)
        })
        .collect()
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
mod tests;
