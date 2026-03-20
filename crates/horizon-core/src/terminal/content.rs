use alacritty_terminal::term::cell::{Cell, Flags};

use super::{
    Column, Dimensions, PathBuf, RenderableContent, Scroll, TermDamage, Terminal, current_cwd_for_pid,
    find_file_path_at_column, find_url_at_column,
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

    /// Extract all text from the terminal grid including scrollback history.
    ///
    /// Returns lines from oldest (top of scrollback) to newest (bottom of
    /// screen). Each line is trimmed of trailing whitespace. The extraction
    /// locks the terminal mutex once and copies text in a single pass.
    #[must_use]
    pub fn full_text_lines(&self, max_lines: usize) -> Vec<String> {
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

        lines
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

    /// Return a clickable target (URL or file path) at the given
    /// viewport-relative row and column, if any is detected.
    #[must_use]
    pub fn clickable_at_point(&self, row: usize, col: usize) -> Option<String> {
        let term = self.term.lock();
        let content = term.renderable_content();
        let cols = usize::from(self.cols);
        let mut line_chars: Vec<char> = vec![' '; cols];

        for indexed in content.display_iter {
            let Ok(rendered_row) = usize::try_from(indexed.point.line.0) else {
                continue;
            };
            if rendered_row != row {
                continue;
            }
            let column = indexed.point.column.0;
            if column < cols {
                line_chars[column] = indexed.cell.c;
            }
        }

        find_url_at_column(&line_chars, col).or_else(|| find_file_path_at_column(&line_chars, col))
    }
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
    use alacritty_terminal::term::cell::{Cell, Flags};

    use super::append_cell_text;

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
}
