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
    /// Components in draw order.
    pub components: Vec<Component>,
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
