//! The count that one panel adds to the shutdown of its board.
//!
//! A panel is shut down by whichever of its parts finishes first: the program of
//! its terminal has ended, the work of its agent was set aside, or the join of
//! its event loop returned. Each part holds a clone of this value, and the first
//! one to finish counts the panel, so a second part never counts it again.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The one count that one panel adds to a shutdown when it is shut down.
#[derive(Clone)]
pub(crate) struct PanelCompletion {
    finished: Arc<AtomicBool>,
    completed: Arc<AtomicUsize>,
}

impl PanelCompletion {
    #[must_use]
    pub(crate) fn new(completed: &Arc<AtomicUsize>) -> Self {
        Self {
            finished: Arc::new(AtomicBool::new(false)),
            completed: Arc::clone(completed),
        }
    }

    /// Counts the panel of this value. A later call does nothing.
    pub(crate) fn finish(&self) {
        if !self.finished.swap(true, Ordering::AcqRel) {
            self.completed.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    use super::PanelCompletion;

    #[test]
    fn the_parts_of_one_panel_count_it_once() {
        let completed = Arc::new(AtomicUsize::new(0));
        let first = PanelCompletion::new(&completed);
        let second = first.clone();

        first.finish();
        first.finish();
        second.finish();

        assert_eq!(completed.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[test]
    fn two_panels_count_twice() {
        let completed = Arc::new(AtomicUsize::new(0));

        PanelCompletion::new(&completed).finish();
        PanelCompletion::new(&completed).finish();

        assert_eq!(completed.load(std::sync::atomic::Ordering::Relaxed), 2);
    }
}
