//! How a board is split into the rows of the runtime index and put together again.
//! Each row keeps its value as JSON. A value that JSON cannot carry exactly, such as
//! a position that is not a number, is kept as YAML, as `runtime.yaml` keeps it.
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{Error, Result};
use crate::runtime_state::{PanelState, RuntimeState, WorkspaceState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Format {
    Json,
    Yaml,
}

impl Format {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Yaml => "yaml",
        }
    }

    pub(super) fn parse(text: &str) -> Result<Self> {
        match text {
            "json" => Ok(Self::Json),
            "yaml" => Ok(Self::Yaml),
            other => Err(Error::State(format!("unknown runtime index row format {other:?}"))),
        }
    }
}

/// A value as one row of the index stores it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Encoded {
    pub format: Format,
    pub data: String,
}

impl Encoded {
    /// The JSON text of `value`, before it is known to read back.
    pub(super) fn json<T: Serialize>(value: &T) -> Result<String> {
        serde_json::to_string(value).map_err(|error| Error::State(error.to_string()))
    }

    /// The row for `value`, given its JSON text: the JSON when it reads back as the
    /// same type, YAML otherwise.
    pub(super) fn settle<T: Serialize + DeserializeOwned>(value: &T, json: String) -> Result<Self> {
        if serde_json::from_str::<T>(&json).is_ok() {
            return Ok(Self {
                format: Format::Json,
                data: json,
            });
        }
        let data = serde_yaml::to_string(value).map_err(|error| Error::State(error.to_string()))?;
        Ok(Self {
            format: Format::Yaml,
            data,
        })
    }

    pub(super) fn decode<T: DeserializeOwned>(&self) -> Result<T> {
        match self.format {
            Format::Json => serde_json::from_str(&self.data).map_err(|error| Error::State(error.to_string())),
            Format::Yaml => serde_yaml::from_str(&self.data).map_err(|error| Error::State(error.to_string())),
        }
    }
}

/// A board split into the rows of the index.
pub(super) struct BoardRows<'a> {
    /// The board without its workspaces.
    pub board: RuntimeState,
    pub workspaces: Vec<WorkspaceRow<'a>>,
}

pub(super) struct WorkspaceRow<'a> {
    /// The workspace without its panels.
    pub state: WorkspaceState,
    pub panels: &'a [PanelState],
}

impl<'a> BoardRows<'a> {
    pub(super) fn new(state: &'a RuntimeState) -> Self {
        Self {
            board: without_workspaces(state),
            workspaces: state
                .workspaces
                .iter()
                .map(|workspace| WorkspaceRow {
                    state: without_panels(workspace),
                    panels: &workspace.panels,
                })
                .collect(),
        }
    }
}

/// Puts a board together from its rows. Workspaces come in order; each panel names
/// the position of its workspace and comes in order within it.
///
/// # Errors
/// Returns an error if a panel names a workspace that the index does not have.
pub(super) fn assemble(
    mut board: RuntimeState,
    workspaces: Vec<WorkspaceState>,
    panels: Vec<(usize, PanelState)>,
) -> Result<RuntimeState> {
    board.workspaces = workspaces;
    for (workspace, panel) in panels {
        board
            .workspaces
            .get_mut(workspace)
            .ok_or_else(|| Error::State(format!("runtime index panel names missing workspace {workspace}")))?
            .panels
            .push(panel);
    }
    Ok(board)
}

/// Every field is named, so a field added to the board must be handled here.
fn without_workspaces(state: &RuntimeState) -> RuntimeState {
    let RuntimeState {
        version,
        window,
        canvas_view,
        pan_offset,
        active_workspace_local_id,
        focused_panel_local_id,
        detached_workspaces,
        workspaces: _,
        browser,
    } = state;
    RuntimeState {
        version: *version,
        window: window.clone(),
        canvas_view: *canvas_view,
        pan_offset: *pan_offset,
        active_workspace_local_id: active_workspace_local_id.clone(),
        focused_panel_local_id: focused_panel_local_id.clone(),
        detached_workspaces: detached_workspaces.clone(),
        workspaces: Vec::new(),
        browser: browser.clone(),
    }
}

/// Every field is named, so a field added to a workspace must be handled here.
fn without_panels(workspace: &WorkspaceState) -> WorkspaceState {
    let WorkspaceState {
        local_id,
        name,
        cwd,
        position,
        place,
        template,
        layout,
        panels: _,
    } = workspace;
    WorkspaceState {
        local_id: local_id.clone(),
        name: name.clone(),
        cwd: cwd.clone(),
        position: *position,
        place: place.clone(),
        template: template.clone(),
        layout: *layout,
        panels: Vec::new(),
    }
}
