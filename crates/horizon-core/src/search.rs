use horizon_terminal::search_lines;
pub use horizon_terminal::{SearchMatch, SearchOptions};

use super::board::Board;
use super::panel::PanelId;

/// All matches found in a single panel.
#[derive(Clone, Debug)]
pub struct PanelSearchResult {
    pub panel_id: PanelId,
    pub panel_title: String,
    /// The extracted lines of text from this terminal.
    pub lines: Vec<String>,
    pub matches: Vec<SearchMatch>,
    /// Total grid lines (scrollback + screen) at snapshot time, before
    /// trailing-empty-line trimming.  Used to map a `line_index` back to
    /// a scrollback offset.
    pub total_lines: usize,
}

/// Aggregated search results across all panels.
#[derive(Clone, Debug, Default)]
pub struct SearchResults {
    pub panels: Vec<PanelSearchResult>,
    pub total_matches: usize,
}

/// Maximum number of scrollback + screen lines to extract per terminal.
/// Keeps search responsive even with deep scrollback buffers.
const MAX_EXTRACT_LINES: usize = 10_000;

/// Extract all text from a terminal including scrollback history.
///
/// Delegates to `Terminal::full_text_lines` which reads the entire grid
/// (scrollback + screen) in a single mutex lock.
fn extract_terminal_lines(board: &Board, panel_id: PanelId) -> (Vec<String>, usize) {
    let Some(panel) = board.panel(panel_id) else {
        return (Vec::new(), 0);
    };
    let Some(terminal) = panel.terminal() else {
        return (Vec::new(), 0);
    };

    terminal.full_text_lines(MAX_EXTRACT_LINES)
}

/// Search across all terminal panels on the board.
///
/// Returns structured results grouped by panel, with line context for
/// each match. The search is performed on extracted text snapshots,
/// so it does not hold terminal locks during matching.
#[must_use]
pub fn search_board(board: &Board, query: &str, options: &SearchOptions) -> SearchResults {
    if query.is_empty() {
        return SearchResults::default();
    }

    let panel_ids: Vec<(PanelId, String)> = board
        .panels
        .iter()
        .filter(|p| p.terminal().is_some())
        .map(|p| (p.id, p.display_title().into_owned()))
        .collect();

    let mut results = SearchResults::default();

    for (panel_id, panel_title) in panel_ids {
        let (lines, total_lines) = extract_terminal_lines(board, panel_id);
        let matches = search_lines(&lines, query, options);
        if !matches.is_empty() {
            results.total_matches += matches.len();
            results.panels.push(PanelSearchResult {
                panel_id,
                panel_title,
                lines,
                matches,
                total_lines,
            });
        }
    }

    results
}
