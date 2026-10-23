//! The size of a terminal in the tests: a screen with no history of its own.

use alacritty_terminal::grid::Dimensions;

pub(crate) struct TestSize {
    rows: usize,
    cols: usize,
}

impl TestSize {
    pub(crate) fn new(rows: usize, cols: usize) -> Self {
        Self { rows, cols }
    }
}

impl Dimensions for TestSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}
