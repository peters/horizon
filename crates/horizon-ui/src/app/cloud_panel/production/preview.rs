//! What the synthetic clouds of a debug build share: when a session takes one, and a
//! member panel that waits for its cloud.
use crate::app::HorizonApp;
use horizon_core::{CloudWait, Panel, PanelKind, PanelOptions, WorkspaceId};

/// A saved session would persist a synthetic cloud, and removal would then refuse
/// it. Any cloud already on the board is left alone.
pub(super) fn accepts(app: &HorizonApp) -> bool {
    app.active_session.as_ref().is_some_and(|session| !session.persistent) && app.cloud_prototype.groups.0.is_empty()
}

/// A member panel named `name` that waits as a restored member does while Horizon
/// reconnects its cloud, so the cloud shows its panels, not its body.
pub(super) fn waiting_member(app: &mut HorizonApp, workspace: WorkspaceId, name: &str) -> Option<String> {
    // The panel is replaced by its placeholder at once; its command only has to exit.
    let (command, args) = if cfg!(windows) {
        ("cmd.exe", vec!["/C".to_owned(), "exit".to_owned()])
    } else {
        ("/bin/sh", vec!["-c".to_owned(), "exit".to_owned()])
    };
    let options = || PanelOptions {
        name: Some(name.into()),
        kind: PanelKind::Shell,
        command: Some(command.into()),
        args: args.clone(),
        ..PanelOptions::default()
    };
    let id = app.board.create_panel(options(), workspace).ok()?;
    let panel = app.board.panel_mut(id)?;
    let member = panel.local_id.clone();
    let placeholder = Panel::cloud_placeholder(
        id,
        workspace,
        PanelOptions {
            local_id: Some(member.clone()),
            ..options()
        },
        CloudWait::Reconnecting,
    )
    .ok()?;
    panel.request_shutdown();
    *panel = placeholder;
    Some(member)
}
