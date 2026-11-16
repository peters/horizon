//! The description of the panel that a holder owns, and the document that
//! carries it.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;

use alacritty_terminal::event::WindowSize;
use serde::{Deserialize, Serialize};

use super::HolderError;

/// The largest launch document of a holder.
pub(crate) const MAX_DOCUMENT: u64 = 1024 * 1024;

/// The panel that a holder owns: its program, its folder, its environment and the
/// first size of its terminal.
///
/// The document reaches the holder on its standard input, never in its arguments:
/// the environment of an agent panel holds secrets, and the arguments of a
/// process are visible to every process of the computer. A field that this build
/// does not know is refused, so a document of another version never starts a
/// program with a launch that this build reads only in part.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Launch {
    /// The identity of the panel in its Horizon home, which names its socket.
    pub local_id: String,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub rows: u16,
    pub cols: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

impl Launch {
    /// The terminal size that this launch opens.
    #[must_use]
    pub(crate) fn window_size(&self) -> WindowSize {
        WindowSize {
            num_lines: self.rows.max(1),
            num_cols: self.cols.max(2),
            cell_width: self.cell_width.max(1),
            cell_height: self.cell_height.max(1),
        }
    }

    /// The document that carries this launch to a holder.
    ///
    /// # Errors
    ///
    /// Returns why the document cannot be written, or that it is too large.
    pub(crate) fn document(&self) -> Result<Vec<u8>, HolderError> {
        let document = serde_json::to_vec(self)?;
        if document.len() as u64 > MAX_DOCUMENT {
            return Err(HolderError::DocumentTooLong);
        }
        Ok(document)
    }

    /// Reads the launch document of a holder from `reader`.
    ///
    /// # Errors
    ///
    /// Returns why the document cannot be read, or that it is not one document of
    /// this build.
    pub(crate) fn read(reader: &mut impl Read) -> Result<Self, HolderError> {
        let mut document = Vec::new();
        reader.take(MAX_DOCUMENT + 1).read_to_end(&mut document)?;
        if document.len() as u64 > MAX_DOCUMENT {
            return Err(HolderError::DocumentTooLong);
        }
        Ok(serde_json::from_slice(&document)?)
    }
}
