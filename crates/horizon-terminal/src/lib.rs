//! Click targets and search on an alacritty terminal grid, for any UI or server.
//!
//! [`target_at`] finds what a click on one cell opens: an OSC 8 hyperlink first, then a URL on the
//! logical line of the cell, then a file path on its soft-wrapped line. The logical line also joins
//! rows that a program or a multiplexer hard-wrapped through the middle of a URL. A cell is a grid
//! [`Point`], so its line can be in the history as well as on the screen. [`search_lines`] finds a
//! literal or regex query in lines of text.
//!
//! The crate has no PTY and no UI toolkit, and it depends on no other Horizon crate.
#![forbid(unsafe_code)]

mod links;
mod logical_line;
mod search;
#[cfg(test)]
mod test_size;

use alacritty_terminal::index::Point;
use alacritty_terminal::term::Term;

pub use links::hyperlink_at;
pub use search::{MAX_MATCHES, SearchMatch, SearchOptions, search_lines};

use logical_line::{RowJoin, logical_line_at};

/// What a click on a cell opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// The URI of an OSC 8 hyperlink.
    Hyperlink(String),
    /// An `https://`, `http://` or `file://` URL in the text.
    Url(String),
    /// An absolute or home-relative file path in the text.
    Path(String),
}

impl Target {
    /// The URI, URL or path.
    #[must_use]
    pub fn into_text(self) -> String {
        match self {
            Self::Hyperlink(text) | Self::Url(text) | Self::Path(text) => text,
        }
    }
}

/// The target under the cell at `point`. OSC 8 hyperlinks take priority over URL or path text. URL
/// text may continue across rows that a program hard-wrapped; paths follow only the terminal's own
/// soft wraps.
#[must_use]
pub fn target_at<T>(term: &Term<T>, point: Point) -> Option<Target> {
    if let Some(hyperlink) = hyperlink_at(term, point) {
        return Some(Target::Hyperlink(hyperlink.uri().to_owned()));
    }
    // Hard-wrap joints only ever drop blank cells, so a click they leave without a position has no
    // path under it either.
    let line = logical_line_at(term, point, RowJoin::UrlHardWraps)?;
    if let Some(url) = links::find_url_at_column(&line.chars, line.column) {
        return Some(Target::Url(url));
    }
    let line = if line.joins_hard_wraps {
        logical_line_at(term, point, RowJoin::SoftWraps)?
    } else {
        line
    };
    links::find_file_path_at_column(&line.chars, line.column).map(Target::Path)
}
