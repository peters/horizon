use super::{Column, Point, Selection, SelectionType, Side, Terminal, viewport_to_point};

impl Terminal {
    /// Start a new text selection at the given viewport-relative row and column.
    pub fn start_selection(&self, sel_type: SelectionType, row: usize, col: usize) {
        let mut term = self.term.lock();
        let display_offset = term.grid().display_offset();
        let point = viewport_to_point(display_offset, Point::new(row, Column(col)));
        let side = Side::Left;
        term.selection = Some(Selection::new(sel_type, point, side));
    }

    /// Update the active selection to the given viewport-relative row and column.
    pub fn update_selection(&self, row: usize, col: usize, side: Side) {
        let mut term = self.term.lock();
        let display_offset = term.grid().display_offset();
        let point = viewport_to_point(display_offset, Point::new(row, Column(col)));
        if let Some(selection) = term.selection.as_mut() {
            selection.update(point, side);
            selection.include_all();
        }
    }

    /// Clear any active selection.
    pub fn clear_selection(&self) {
        self.term.lock().selection = None;
    }

    /// Return whether a selection is currently active.
    #[must_use]
    pub fn has_selection(&self) -> bool {
        self.term.lock().selection.is_some()
    }

    /// Extract the currently selected text, if any.
    #[must_use]
    pub fn selection_to_string(&self) -> Option<String> {
        self.term.lock().selection_to_string()
    }
}
