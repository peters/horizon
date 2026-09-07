//! Cache presentation only on explicit interaction; admission still checks the current view.

use super::{Board, Context, PanelId, RemoteEnvironmentSummary};
use horizon_core::{
    Panel, PanelKind, PanelResume, SshConnectionStatus, remote_panel_attachment::RemotePanelTerminalSize,
};

pub(super) struct View {
    pub(super) id: PanelId,
    pub(super) label: String,
}

pub(super) struct Request {
    pub(super) local_id: String,
    pub(super) terminal: RemotePanelTerminalSize,
}

pub(super) fn list(
    board: &Board,
    selected: Option<&RemoteEnvironmentSummary>,
    owner: Option<&str>,
) -> Result<Vec<View>, &'static str> {
    let owner = owner.ok_or("Open the owning persistent session before reconnecting its saved views.")?;
    let selected = selected.ok_or("Select a saved environment first.")?;
    if owner != selected.owning_session_id {
        return Err("This environment belongs to another session. Open that session before reconnecting.");
    }
    Ok(board
        .panels
        .iter()
        .filter(|panel| matches(panel, selected, owner))
        .map(|panel| View {
            id: panel.id,
            label: format!("{} · {}", panel.title, panel.local_id),
        })
        .collect())
}

fn matches(panel: &Panel, selected: &RemoteEnvironmentSummary, owner: &str) -> bool {
    owner == selected.owning_session_id
        && panel.remote_workspace().is_some_and(|reference| {
            reference.owner_session_id() == owner && reference.workspace_local_id() == selected.workspace_local_id
        })
}

pub(super) fn request(
    board: &Board,
    selected: &RemoteEnvironmentSummary,
    owner: &str,
    id: PanelId,
    ctx: &Context,
) -> Result<Request, &'static str> {
    let panel = board
        .panel(id)
        .filter(|panel| matches(panel, selected, owner))
        .ok_or("The selected remote view changed. Show the session panels again.")?;
    if panel.kind != PanelKind::Ssh || panel.resume != PanelResume::Fresh || panel.session_binding.is_some() {
        return Err("This view is not a supported remote terminal.");
    }
    let terminal = panel.terminal().ok_or("This view no longer contains a terminal.")?;
    if panel.ssh_status() != Some(SshConnectionStatus::Disconnected) || !terminal.child_exited() {
        return Err("This view still has a local connection. It was not replaced.");
    }
    let viewport = crate::terminal_widget::viewport_for_available_space(ctx, egui::Vec2::ZERO);
    Ok(Request {
        local_id: panel.local_id.clone(),
        terminal: RemotePanelTerminalSize {
            rows: terminal.rows(),
            cols: terminal.cols(),
            cell_width: viewport.cell_width,
            cell_height: viewport.cell_height,
            scrollback_limit: terminal.scrollback_limit(),
            window_id: id.0,
            kitty_keyboard: true,
        },
    })
}
