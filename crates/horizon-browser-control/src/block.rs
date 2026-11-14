//! A block document: an agent-drawn card in the person's workspace.
//!
//! An agent asks Horizon to show a card of its own making — a machine it
//! holds, a plan, a list of results — and sends this document for the body.
//! The document is a flat list of components in draw order: a heading, body
//! text, label and value pairs, a badge, a progress bar, a rule, selectable
//! rows, buttons and a link, a caption, the steps of a longer piece of work, a
//! board of waves and workers, a callout and a log. It has no inputs, images or
//! tables, and no field that Horizon fetches or opens by itself: a link opens in
//! a browser panel only when the person presses it. The document asks for a
//! narrow or a wide card; the card draws the form that its real width fits.
//!
//! The document holds components only. The host assigns the revision, keeps
//! the card in its own state and reports a click on a button or a row back to
//! the agent that owns it. [`parse_document`] is the single entry point for a
//! document that arrives from outside Horizon: it checks the size, the shape
//! and every rule of [`validate_document`] before the UI can draw anything.

mod events;
mod link;
mod model;
mod validate;

pub use events::ClickEvent;
pub use link::{checked_link, link_host};
pub use model::{
    Badge, BlockDocument, BoardColumn, BoardItem, BoardRow, BoardState, Button, Component, KvRow, ListLayout, ListRow,
    LogEntry, Selection, Step, StepState, Tone, Width,
};
pub use validate::{BlockDocumentError, parse_document, validate_document};

#[cfg(test)]
mod tests;
