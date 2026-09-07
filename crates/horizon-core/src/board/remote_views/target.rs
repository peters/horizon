//! Metadata-only target selection and persistence checks; no terminal cwd reads.

use super::{Board, PanelKind, RemoteViewReopenError as Error, RemoteWorkspaceReference};
use crate::{PanelState, RuntimeState, WorkspaceId, WorkspaceState, new_local_id};
use std::collections::HashSet;

pub(super) enum ViewTarget {
    Existing { id: WorkspaceId, local_id: String },
    New { id: WorkspaceId, local_id: String },
}

impl ViewTarget {
    pub(super) fn select(board: &Board, reference: &RemoteWorkspaceReference) -> Result<Self, Error> {
        let mut matching = board
            .workspaces
            .iter()
            .filter(|workspace| workspace.remote_workspace.as_ref() == Some(reference));
        let first = matching.next();
        if matching.next().is_some() {
            return Err(Error::AmbiguousWorkspace);
        }
        Ok(first.map_or_else(
            || Self::New {
                id: WorkspaceId(board.next_workspace_id),
                local_id: new_local_id(),
            },
            |workspace| Self::Existing {
                id: workspace.id,
                local_id: workspace.local_id.clone(),
            },
        ))
    }

    pub(super) fn id(&self) -> WorkspaceId {
        match self {
            Self::Existing { id, .. } | Self::New { id, .. } => *id,
        }
    }

    pub(super) fn check(
        &self,
        board: &Board,
        reference: &RemoteWorkspaceReference,
        panel_id: &str,
    ) -> Result<(), Error> {
        match (self, Self::select(board, reference)?) {
            (
                Self::Existing { id, local_id },
                Self::Existing {
                    id: current,
                    local_id: current_local,
                },
            ) if *id == current && *local_id == current_local => {}
            (Self::New { id, .. }, Self::New { id: current, .. }) if *id == current && id.0 != u64::MAX => {}
            _ => return Err(Error::TargetChanged),
        }
        let mut numeric_workspaces = HashSet::new();
        let mut numeric_panels = HashSet::new();
        let mut state = RuntimeState::default();
        for workspace in &board.workspaces {
            if !numeric_workspaces.insert(workspace.id) {
                return Err(Error::TargetChanged);
            }
            let mut saved = WorkspaceState {
                local_id: workspace.local_id.clone(),
                remote_workspace: workspace.remote_workspace.clone(),
                ..WorkspaceState::default()
            };
            for id in &workspace.panels {
                let panel = board.panel(*id).ok_or(Error::TargetChanged)?;
                if !numeric_panels.insert(*id) || panel.workspace_id != workspace.id {
                    return Err(Error::TargetChanged);
                }
                saved.panels.push(PanelState {
                    local_id: panel.local_id.clone(),
                    kind: panel.kind,
                    resume: panel.resume.clone(),
                    session_binding: panel.session_binding.clone(),
                    remote_workspace: panel.remote_workspace().cloned(),
                    ..PanelState::default()
                });
            }
            state.workspaces.push(saved);
        }
        if numeric_panels.len() != board.panels.len() || numeric_panels.contains(&crate::PanelId(board.next_panel_id)) {
            return Err(Error::TargetChanged);
        }
        let index = match self {
            Self::Existing { id, .. } => board
                .workspaces
                .iter()
                .position(|workspace| workspace.id == *id)
                .ok_or(Error::TargetChanged)?,
            Self::New { id, local_id } => {
                if numeric_workspaces.contains(id) {
                    return Err(Error::TargetChanged);
                }
                state.workspaces.push(WorkspaceState {
                    local_id: local_id.clone(),
                    remote_workspace: Some(reference.clone()),
                    ..WorkspaceState::default()
                });
                state.workspaces.len() - 1
            }
        };
        state.workspaces[index].panels.push(PanelState {
            local_id: panel_id.into(),
            kind: PanelKind::Ssh,
            remote_workspace: Some(reference.clone()),
            ..PanelState::default()
        });
        state.validate_remote_references().map_err(|_| Error::TargetChanged)
    }

    pub(super) fn install(
        self,
        board: &mut Board,
        reference: &RemoteWorkspaceReference,
        repository: &str,
    ) -> WorkspaceId {
        match self {
            Self::Existing { id, .. } => id,
            Self::New { id, local_id } => {
                let created = board.create_workspace(repository);
                if let Some(workspace) = board.workspace_mut(created) {
                    workspace.local_id = local_id;
                    workspace.remote_workspace = Some(reference.clone());
                }
                id
            }
        }
    }
}
