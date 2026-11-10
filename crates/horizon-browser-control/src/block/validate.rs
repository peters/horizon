//! Bounds and rules for a block document.
//!
//! Horizon draws agent-authored text inside a person's workspace, so every
//! value is checked before any UI sees it: the document is bounded in size and
//! component count, a text is one line except the body text of a `text`
//! component, and ids are short, machine-readable and unique in the whole
//! document. A rejected document names the place that broke the rule, such as
//! `components[3].items[1].id`.

use std::collections::HashSet;

use thiserror::Error;

use super::model::{BlockDocument, Button, Component, KvRow, ListRow, Selection};

impl BlockDocument {
    /// Largest serialized document accepted, in bytes.
    pub const MAX_SERIALIZED_BYTES: usize = 32 * 1024;
    /// Most components one document may hold.
    pub const MAX_COMPONENTS: usize = 64;
    /// Most rows one `kv` component may hold.
    pub const MAX_KV_ROWS: usize = 24;
    /// Most rows one `list` component may hold.
    pub const MAX_LIST_ROWS: usize = 32;
    /// Most items one `buttons` component may hold.
    pub const MAX_BUTTONS: usize = 8;
    /// Most characters one text may hold.
    pub const MAX_TEXT_CHARS: usize = 512;
    /// Most characters one id may hold.
    pub const MAX_ID_CHARS: usize = 64;
    /// Largest percentage a progress component may show.
    pub const MAX_PERCENT: u8 = 100;
}

/// Why a block document was rejected.
///
/// Every variant that comes from the document content names the offending
/// place, and [`BlockDocumentError::code`] turns the variant into a stable
/// machine-readable code.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum BlockDocumentError {
    /// The serialized document exceeds the byte limit, so parsing never ran.
    #[error("block document is {actual} bytes, above the {limit} byte limit")]
    DocumentTooLarge {
        /// Measured serialized length in bytes.
        actual: usize,
        /// Accepted length in bytes.
        limit: usize,
    },
    /// Serde rejected the shape: a missing field, an unknown field, an unknown
    /// component type or an unknown enum value.
    #[error("block document is not valid JSON: {message}")]
    InvalidJson {
        /// The serde message.
        message: String,
    },
    /// A component list, row list or button list is too long.
    #[error("{path} has {actual} items, above the {limit} item limit")]
    TooManyItems {
        /// Offending place.
        path: String,
        /// Measured count.
        actual: usize,
        /// Accepted count.
        limit: usize,
    },
    /// A text is too long.
    #[error("{path} has {actual} characters, above the {limit} character limit")]
    TextTooLong {
        /// Offending place.
        path: String,
        /// Measured character count.
        actual: usize,
        /// Accepted character count.
        limit: usize,
    },
    /// A text holds a control character. A line break is only valid in the body
    /// text of a `text` component.
    #[error("{path} contains the control character U+{code_point:04X}")]
    ControlCharacter {
        /// Offending place.
        path: String,
        /// Unicode scalar value of the character.
        code_point: u32,
    },
    /// A heading, label or title is empty.
    #[error("{path} cannot be empty")]
    EmptyText {
        /// Offending place.
        path: String,
    },
    /// An id is not `[a-z0-9_.-]{1,64}`.
    #[error("{path} is not a valid id: {id}")]
    InvalidId {
        /// Offending place.
        path: String,
        /// The rejected id.
        id: String,
    },
    /// An id is already used elsewhere in the document.
    #[error("{path} repeats the id {id}")]
    DuplicateId {
        /// Offending place.
        path: String,
        /// The repeated id.
        id: String,
    },
    /// A progress percentage is above 100.
    #[error("{path} is {percent}, above the {limit} percent limit")]
    PercentOutOfRange {
        /// Offending place.
        path: String,
        /// The rejected percentage.
        percent: u8,
        /// Accepted maximum.
        limit: u8,
    },
    /// A row is selected by default in a list that the person cannot select a
    /// row of.
    #[error("{path} is selected, but the list does not let the person select a row")]
    RowSelectedInUnselectableList {
        /// Offending place.
        path: String,
    },
    /// More than one row of one list is selected by default.
    #[error("{path} is the second selected row of its list; {first} is already selected")]
    DuplicateSelectedRow {
        /// Offending place.
        path: String,
        /// The row that is selected first.
        first: String,
    },
}

impl BlockDocumentError {
    /// Stable machine-readable code for a tool outcome.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::DocumentTooLarge { .. } => "document_too_large",
            _ => "invalid_document",
        }
    }
}

/// Parse and validate a document an agent sent.
///
/// # Errors
/// Returns [`BlockDocumentError::DocumentTooLarge`] when the string is above
/// [`BlockDocument::MAX_SERIALIZED_BYTES`], and a rejection from
/// [`validate_document`] or [`BlockDocumentError::InvalidJson`] when the
/// document does not parse.
pub fn parse_document(json: &str) -> Result<BlockDocument, BlockDocumentError> {
    if json.len() > BlockDocument::MAX_SERIALIZED_BYTES {
        return Err(BlockDocumentError::DocumentTooLarge {
            actual: json.len(),
            limit: BlockDocument::MAX_SERIALIZED_BYTES,
        });
    }
    let document: BlockDocument = serde_json::from_str(json).map_err(|error| BlockDocumentError::InvalidJson {
        message: error.to_string(),
    })?;
    validate_document(&document)?;
    Ok(document)
}

/// Check the rules that serde cannot express.
///
/// # Errors
/// Describes the first violation with the place that caused it.
pub fn validate_document(document: &BlockDocument) -> Result<(), BlockDocumentError> {
    if document.components.len() > BlockDocument::MAX_COMPONENTS {
        return Err(BlockDocumentError::TooManyItems {
            path: "components".to_owned(),
            actual: document.components.len(),
            limit: BlockDocument::MAX_COMPONENTS,
        });
    }
    let mut ids = HashSet::new();
    for (index, component) in document.components.iter().enumerate() {
        validate_component(index, component, &mut ids)?;
    }
    Ok(())
}

fn validate_component<'a>(
    index: usize,
    component: &'a Component,
    ids: &mut HashSet<&'a str>,
) -> Result<(), BlockDocumentError> {
    let at = format!("components[{index}]");
    match component {
        Component::Heading { text, subtitle, badge } => {
            filled_line(&field(&at, "text"), text)?;
            if let Some(subtitle) = subtitle {
                line(&field(&at, "subtitle"), subtitle)?;
            }
            if let Some(badge) = badge {
                line(&field(&format!("{at}.badge"), "text"), &badge.text)?;
            }
            Ok(())
        }
        Component::Text { text } => body_text(&field(&at, "text"), text),
        Component::Kv { rows } => {
            check_count(&field(&at, "rows"), rows.len(), BlockDocument::MAX_KV_ROWS)?;
            for (row_index, row) in rows.iter().enumerate() {
                validate_kv_row(&element(&at, "rows", row_index), row)?;
            }
            Ok(())
        }
        Component::Badge { text, .. } => line(&field(&at, "text"), text),
        Component::Progress { label, percent, detail } => {
            filled_line(&field(&at, "label"), label)?;
            if let Some(detail) = detail {
                line(&field(&at, "detail"), detail)?;
            }
            if *percent > BlockDocument::MAX_PERCENT {
                return Err(BlockDocumentError::PercentOutOfRange {
                    path: field(&at, "percent"),
                    percent: *percent,
                    limit: BlockDocument::MAX_PERCENT,
                });
            }
            Ok(())
        }
        Component::Divider => Ok(()),
        Component::List { selection, rows } => {
            check_count(&field(&at, "rows"), rows.len(), BlockDocument::MAX_LIST_ROWS)?;
            let mut selected: Option<String> = None;
            for (row_index, row) in rows.iter().enumerate() {
                let row_at = element(&at, "rows", row_index);
                validate_list_row(&row_at, row, ids)?;
                if !row.selected {
                    continue;
                }
                let path = field(&row_at, "selected");
                if *selection != Selection::Single {
                    return Err(BlockDocumentError::RowSelectedInUnselectableList { path });
                }
                if let Some(first) = &selected {
                    return Err(BlockDocumentError::DuplicateSelectedRow {
                        path,
                        first: first.clone(),
                    });
                }
                selected = Some(path);
            }
            Ok(())
        }
        Component::Buttons { items } => {
            check_count(&field(&at, "items"), items.len(), BlockDocument::MAX_BUTTONS)?;
            for (item_index, item) in items.iter().enumerate() {
                validate_button(&element(&at, "items", item_index), item, ids)?;
            }
            Ok(())
        }
    }
}

fn validate_kv_row(at: &str, row: &KvRow) -> Result<(), BlockDocumentError> {
    filled_line(&field(at, "label"), &row.label)?;
    line(&field(at, "value"), &row.value)
}

fn validate_list_row<'a>(at: &str, row: &'a ListRow, ids: &mut HashSet<&'a str>) -> Result<(), BlockDocumentError> {
    take_id(&field(at, "id"), &row.id, ids)?;
    filled_line(&field(at, "title"), &row.title)?;
    if let Some(subtitle) = &row.subtitle {
        line(&field(at, "subtitle"), subtitle)?;
    }
    Ok(())
}

fn validate_button<'a>(at: &str, button: &'a Button, ids: &mut HashSet<&'a str>) -> Result<(), BlockDocumentError> {
    take_id(&field(at, "id"), &button.id, ids)?;
    filled_line(&field(at, "label"), &button.label)
}

fn field(at: &str, name: &str) -> String {
    format!("{at}.{name}")
}

fn element(at: &str, name: &str, index: usize) -> String {
    format!("{at}.{name}[{index}]")
}

fn check_count(path: &str, actual: usize, limit: usize) -> Result<(), BlockDocumentError> {
    if actual > limit {
        return Err(BlockDocumentError::TooManyItems {
            path: path.to_owned(),
            actual,
            limit,
        });
    }
    Ok(())
}

/// A filled single-line text: a heading, a label or a title.
fn filled_line(path: &str, value: &str) -> Result<(), BlockDocumentError> {
    if value.trim().is_empty() {
        return Err(BlockDocumentError::EmptyText { path: path.to_owned() });
    }
    line(path, value)
}

/// A single-line text that may be empty, such as a value or a badge.
fn line(path: &str, value: &str) -> Result<(), BlockDocumentError> {
    check_length(path, value)?;
    reject_controls(path, value, false)
}

/// Body text: single-line fields plus line breaks.
fn body_text(path: &str, value: &str) -> Result<(), BlockDocumentError> {
    check_length(path, value)?;
    reject_controls(path, value, true)
}

fn check_length(path: &str, value: &str) -> Result<(), BlockDocumentError> {
    let actual = value.chars().count();
    if actual > BlockDocument::MAX_TEXT_CHARS {
        return Err(BlockDocumentError::TextTooLong {
            path: path.to_owned(),
            actual,
            limit: BlockDocument::MAX_TEXT_CHARS,
        });
    }
    Ok(())
}

fn reject_controls(path: &str, value: &str, allow_line_break: bool) -> Result<(), BlockDocumentError> {
    for character in value.chars() {
        if character.is_control() && !(allow_line_break && character == '\n') {
            return Err(BlockDocumentError::ControlCharacter {
                path: path.to_owned(),
                code_point: u32::from(character),
            });
        }
    }
    Ok(())
}

/// Check one id and remember it: button ids and list row ids share one
/// namespace across the whole document.
fn take_id<'a>(path: &str, id: &'a str, ids: &mut HashSet<&'a str>) -> Result<(), BlockDocumentError> {
    let valid = !id.is_empty()
        && id.chars().count() <= BlockDocument::MAX_ID_CHARS
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.' | '-'));
    if !valid {
        return Err(BlockDocumentError::InvalidId {
            path: path.to_owned(),
            id: id.to_owned(),
        });
    }
    if !ids.insert(id) {
        return Err(BlockDocumentError::DuplicateId {
            path: path.to_owned(),
            id: id.to_owned(),
        });
    }
    Ok(())
}
