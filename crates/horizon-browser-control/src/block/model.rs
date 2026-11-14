//! Serde types of one block document.
//!
//! A document is a flat list of components in draw order. A component
//! carries plain text only: no markup, no nesting and no field that Horizon
//! fetches or opens. A link carries an address, and Horizon opens it only when
//! the person presses the link. The agent sends the components; the host adds
//! the revision it assigns and the identity of the owning agent.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The card an agent asks Horizon to draw.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BlockDocument {
    /// How wide the card is; the narrow card of today when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<Width>,
    /// Components in draw order.
    pub components: Vec<Component>,
}

/// How wide the card that draws a document is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Width {
    /// The narrow card: one column of components.
    Narrow,
    /// The wide card: a board, a callout beside its buttons and columns of a
    /// log.
    Wide,
}

/// One element of a document. The `type` field selects the variant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Component {
    /// A card title: one short line, an optional second line and an optional
    /// badge drawn on the same row as the title, right aligned.
    Heading {
        /// Title text.
        text: String,
        /// Optional second line under the title.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subtitle: Option<String>,
        /// Optional badge on the title row, right aligned.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        badge: Option<Badge>,
    },
    /// Body text. The only text that may hold line breaks.
    Text {
        /// Body text.
        text: String,
    },
    /// Label and value pairs, drawn in a grid.
    Kv {
        /// The pairs, in draw order.
        rows: Vec<KvRow>,
    },
    /// A short status pill.
    Badge {
        /// Badge text.
        text: String,
        /// Visual weight; neutral when omitted.
        #[serde(default)]
        tone: Tone,
    },
    /// A labelled bar.
    Progress {
        /// Bar label.
        label: String,
        /// Filled percentage, from 0 to 100.
        percent: u8,
        /// Optional text on the label row, right aligned.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// A horizontal rule.
    Divider,
    /// Rows a person can select.
    List {
        /// How many rows the person may select.
        selection: Selection,
        /// The rows, in draw order.
        rows: Vec<ListRow>,
        /// How the rows are drawn; one under another when omitted.
        #[serde(default)]
        layout: ListLayout,
    },
    /// A caption above a group of rows or a choice: upper case, small and
    /// dim, such as `SIZE` or `PLACE`.
    Caption {
        /// The caption text. Horizon draws it in upper case.
        text: String,
    },
    /// The steps of a longer piece of work, each with its state and an
    /// optional short time. A step takes no press.
    Steps {
        /// The steps, in the order they run.
        steps: Vec<Step>,
    },
    /// Buttons a person can press.
    Buttons {
        /// The buttons, in draw order.
        items: Vec<Button>,
    },
    /// A page a person can open in a browser panel.
    Link {
        /// The text of the link, one line.
        label: String,
        /// The absolute `http` or `https` URL that Horizon opens when the
        /// person presses the link.
        url: String,
    },
    /// Who and where the work runs, against the waves that run it: the grid of
    /// the wide card and the grouped list of the narrow one. An item takes no
    /// press.
    Board {
        /// The waves, in the order they run.
        columns: Vec<BoardColumn>,
        /// The workers, in draw order.
        rows: Vec<BoardRow>,
        /// The items of the cells, in draw order.
        items: Vec<BoardItem>,
    },
    /// A strip that asks the person something, with the buttons to answer it.
    /// A button is a button in every sense.
    Callout {
        /// Visual weight; neutral when omitted.
        #[serde(default)]
        tone: Tone,
        /// Optional line above the text, drawn as a caption.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
        /// The question or the statement, one line.
        text: String,
        /// The answers, in draw order.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        buttons: Vec<Button>,
    },
    /// The events of the work, each with its time. An entry takes no press.
    Log {
        /// The entries, in the order they happened.
        entries: Vec<LogEntry>,
    },
}

/// Visual weight of a badge or a button.
///
/// A tone never states Horizon's own status: the card is the agent's view,
/// not the host's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    /// No emphasis.
    #[default]
    Neutral,
    /// An informational hint.
    Info,
    /// A successful state.
    Ok,
    /// A state that needs attention.
    Warn,
    /// A destructive action or a failed state.
    Danger,
    /// The suggested action.
    Primary,
}

/// How many rows of a list a person may select.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Selection {
    /// Rows are shown but cannot be selected.
    None,
    /// One row at a time can be selected.
    Single,
}

/// How a list draws its rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ListLayout {
    /// One row under another, with the mark of the row that is selected.
    #[default]
    Rows,
    /// One row of small pills, for a short choice such as a place. It carries
    /// the titles of its rows only.
    Pills,
}

/// A short status pill beside a heading, or on a row of its own as a
/// [`Component::Badge`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Badge {
    /// Badge text.
    pub text: String,
    /// Visual weight; neutral when omitted.
    #[serde(default)]
    pub tone: Tone,
}

/// One label and value pair.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KvRow {
    /// Left column.
    pub label: String,
    /// Right column.
    pub value: String,
    /// Draw the value in the monospace family; false when omitted.
    #[serde(default)]
    pub mono: bool,
}

/// One selectable row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListRow {
    /// Stable id the agent receives back when the person selects the row.
    pub id: String,
    /// Main line.
    pub title: String,
    /// Optional second line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    /// Colour of the second line; neutral when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<Tone>,
    /// Selected when the card draws this list, until the person selects
    /// another row; false when omitted. Only a list with `selection: single`
    /// may hold a selected row, and at most one row of a list.
    #[serde(default)]
    pub selected: bool,
    /// A short value at the right edge of the row, drawn in the monospace
    /// family: a price or a size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// A short pill at the right edge of the row, after its value: `Ready`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<Badge>,
    /// Drawn as a row that the person cannot choose: a dashed outline and a
    /// softer title, whose second line says what is missing. A press on it
    /// reaches no agent, and it never carries the selection of its list.
    #[serde(default)]
    pub disabled: bool,
}

/// One step of a longer piece of work.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// The line of this step.
    pub label: String,
    /// What the step does now; waiting when omitted.
    #[serde(default)]
    pub state: StepState,
    /// A short time at the right edge, such as `38 s` or `now`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
}

/// What one step of a `steps` component does now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    /// The step finished.
    Done,
    /// The step runs now.
    Now,
    /// The step waits for the ones before it.
    #[default]
    Waiting,
}

/// One wave of a board, drawn as the head of a column.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoardColumn {
    /// The name of the wave, such as `Understand`.
    pub label: String,
    /// A short tag of the column, drawn in a square: `1` or `now`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// A short note beside the label, in the monospace family.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// A short time or count at the right edge, in the monospace family.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// The wave that runs now. At most one column of a board is current.
    #[serde(default)]
    pub current: bool,
}

/// One worker of a board: who the work runs as and where it runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoardRow {
    /// The line of the row, such as `helper-1 · build-1`.
    pub label: String,
    /// A short second line, such as `16 vCPU`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
}

/// One item of a cell of a board.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoardItem {
    /// Index of the column of the item.
    pub column: usize,
    /// Index of the row of the item.
    pub row: usize,
    /// What the item does, one line.
    pub title: String,
    /// What the item does now; waiting when omitted.
    #[serde(default)]
    pub state: BoardState,
    /// A short value at the right edge, in the monospace family.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// The evidence of the item, in the monospace family: one line in the wide
    /// card, the end of the second line in the narrow one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// A short word after the title, such as `added`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

/// What one item of a board does now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BoardState {
    /// The item waits for the item before it.
    #[default]
    Waiting,
    /// The item runs now.
    Now,
    /// The item finished.
    Done,
    /// The item waits for the person.
    NeedsYou,
    /// The item failed.
    Failed,
}

/// One line of a log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LogEntry {
    /// The time of the event, such as `07:31`.
    pub at: String,
    /// What happened, one line.
    pub text: String,
}

/// One button.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Button {
    /// Stable id the agent receives back when the person presses the button.
    pub id: String,
    /// Button text.
    pub label: String,
    /// Visual weight; neutral when omitted.
    #[serde(default)]
    pub tone: Tone,
}
