use super::{Connection, Deployment, Event, HorizonApp, PanelKind, PanelOptions, Settings, cloud_runtime};
use horizon_core::Panel;
use std::path::Path;
use std::{
    sync::{Arc, mpsc::Sender},
    time::Duration,
};

pub(super) fn watch(
    state: &Deployment,
    settings: &Settings,
    root: &Path,
    cancel: &cloud_runtime::Cancellation,
    tx: &Sender<Event>,
    ctx: &egui::Context,
) {
    let result = (|| -> cloud_runtime::Result<()> {
        let worker = state
            .worker
            .as_ref()
            .ok_or(cloud_runtime::Error::Invalid("Cloud has no worker"))?;
        let connection = Connection::new(worker, settings, root)?;
        if state.profile.capabilities.desktop {
            match cloud_runtime::tunnel::DesktopTunnel::open(&connection, cancel) {
                Ok(desktop) => {
                    if tx.send(Event::Desktop(Arc::new(desktop))).is_err() {
                        return Ok(());
                    }
                }
                Err(_) => {
                    let _ = tx.send(Event::Output(
                        "Desktop connection unavailable; agent and browser attachment can continue".into(),
                    ));
                }
            }
        }
        ctx.request_repaint();
        let runner = cloud_runtime::command::Runner {
            cancel,
            emit: &|_| {},
            secrets: vec![],
        };
        if !state.profile.capabilities.desktop && !state.profile.capabilities.browser_tools() {
            return Ok(());
        }
        while !cancel.is_cancelled() {
            let output = runner.run(
                "Cloud presentation discovery",
                &mut connection.command("printf '%s\\n' '{\"operation\":\"list\"}' | horizon-cloud-worker connect"),
                Duration::from_secs(40),
            )?;
            let response: horizon_core::browser::CloudViewResponse = serde_json::from_str(&output)
                .map_err(|_| cloud_runtime::Error::Invalid("Invalid cloud presentation response"))?;
            let _ = tx.send(Event::ClosedBrowsers(response.closed));
            let _ = tx.send(Event::DesktopControl {
                active: response.desktop_controller,
                last: response.desktop_last_input,
            });
            if tx.send(Event::Browsers(response.browsers)).is_err() {
                return Ok(());
            }
            ctx.request_repaint();
            for _ in 0..10 {
                if cancel.is_cancelled() {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = tx.send(Event::failed(error.to_string()));
        ctx.request_repaint();
    }
}
impl super::Runtime {
    fn prepare_attachments(
        &mut self,
        board: &horizon_core::Board,
        group: &horizon_core::cloud_panel::CloudGroup,
    ) -> bool {
        let restore = std::mem::take(&mut self.needs_attach);
        if restore {
            self.pending_session_attachments = self
                .state
                .as_ref()
                .into_iter()
                .flat_map(|state| &state.sessions)
                .filter(|session| board.panel_id_by_local_id(&session.panel_id).is_none())
                .map(|session| session.panel_id.clone())
                .collect();
            self.pending_browser_attachments = group
                .panels
                .iter()
                .filter(|local| {
                    board
                        .panel_id_by_local_id(local)
                        .and_then(|id| board.panel(id))
                        .is_some_and(|panel| panel.kind == PanelKind::Browser)
                })
                .cloned()
                .collect();
        }
        self.pending_browser_attachments.retain(|local| {
            group.panels.contains(local)
                && board
                    .panel_id_by_local_id(local)
                    .and_then(|id| board.panel(id))
                    .is_some_and(|panel| panel.kind == PanelKind::Browser)
        });
        let restore_desktop = std::mem::take(&mut self.needs_desktop);
        if restore || restore_desktop {
            self.pending_member_attachments.extend(
                group
                    .panels
                    .iter()
                    .filter(|local| {
                        board
                            .panel_id_by_local_id(local)
                            .and_then(|id| board.panel(id))
                            .is_some_and(|panel| match panel.kind {
                                PanelKind::Browser => false,
                                PanelKind::Device => restore_desktop,
                                _ => restore,
                            })
                    })
                    .cloned(),
            );
            self.next_attachment_attempt = None;
        }
        let retry = self
            .next_attachment_attempt
            .is_none_or(|at| std::time::Instant::now() >= at);
        if retry {
            self.next_attachment_attempt = Some(std::time::Instant::now() + Duration::from_secs(1));
        }
        retry
    }
}

impl HorizonApp {
    pub(in crate::app) fn close_cloud_browser(&mut self, id: horizon_core::PanelId) -> bool {
        let Some(panel) = self
            .board
            .panel(id)
            .filter(|p| p.kind == PanelKind::Browser && p.browser().is_some())
        else {
            return false;
        };
        let Some(group) = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .find(|g| g.remote.is_some() && g.panels.contains(&panel.local_id))
        else {
            return false;
        };
        let local = panel.local_id.clone();
        let result = (|| -> cloud_runtime::Result<_> {
            let launch = group
                .remote
                .as_ref()
                .ok_or(cloud_runtime::Error::Invalid("Missing cloud"))?;
            let root = self
                .cloud_prototype
                .root
                .as_ref()
                .ok_or(cloud_runtime::Error::Invalid("Cloud is disconnected"))?;
            let runtime =
                self.cloud_prototype
                    .production
                    .runtimes
                    .get(&group.issue)
                    .ok_or(cloud_runtime::Error::Invalid(
                        "Reconnect the cloud before closing its remote browser",
                    ))?;
            let worker = runtime
                .state
                .as_ref()
                .and_then(|s| s.worker.as_ref())
                .ok_or(cloud_runtime::Error::Invalid("Cloud is disconnected"))?;
            let sender = runtime
                .sender
                .clone()
                .ok_or(cloud_runtime::Error::Invalid("Cloud is disconnected"))?;
            let settings = Settings::load(&root.join("settings.json"))?;
            Ok((
                Connection::new(
                    worker,
                    &settings,
                    &cloud_runtime::state::cloud_directory(root, &launch.id)?,
                )?,
                sender,
                runtime.repaint_context.clone(),
            ))
        })();
        match result {
            Ok((connection, sender, context)) => {
                std::thread::spawn(move || {
                    if !local
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                    {
                        return;
                    }
                    let cancel = cloud_runtime::Cancellation::default();
                    let runner = cloud_runtime::command::Runner {
                        cancel: &cancel,
                        emit: &|_| {},
                        secrets: vec![],
                    };
                    let command = format!(
                        "printf '%s\\n' '{{\"operation\":\"close\",\"id\":\"{local}\"}}' | horizon-cloud-worker connect"
                    );
                    let result = runner.run(
                        "Close remote browser",
                        &mut connection.command(&command),
                        Duration::from_secs(40),
                    );
                    let event = match result.and_then(|output| {
                        serde_json::from_str::<horizon_core::browser::CloudViewResponse>(&output)
                            .map_err(|_| cloud_runtime::Error::Invalid("Invalid browser close response"))
                    }) {
                        Ok(response) if response.error.is_none() && response.closed.contains(&local) => {
                            Event::ClosedBrowsers(vec![local])
                        }
                        _ => Event::Output(
                            "Remote browser close was not confirmed. Reconnect and inspect before retrying.".into(),
                        ),
                    };
                    let _ = sender.send(event);
                    if let Some(context) = context {
                        context.request_repaint();
                    }
                });
            }
            Err(error) => self.cloud_prototype.error = Some(error.to_string()),
        }
        true
    }
    pub(super) fn sync_cloud_presentations(&mut self) {
        for index in 0..self.cloud_prototype.groups.0.len() {
            let group = &self.cloud_prototype.groups.0[index];
            if group.remote.is_none() {
                continue;
            }
            let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&group.issue) else {
                continue;
            };
            if runtime
                .state
                .as_ref()
                .is_none_or(|state| state.stage != cloud_runtime::Stage::Ready)
            {
                continue;
            }
            let retry = runtime.prepare_attachments(&self.board, group);
            let members = if retry {
                runtime.pending_member_attachments.clone()
            } else {
                std::collections::HashSet::default()
            };
            let pending_sessions = runtime.pending_session_attachments.clone();
            let pending_browsers = runtime.pending_browser_attachments.clone();
            let discovered = runtime.browsers.is_some();
            let browsers = runtime.browsers.clone().unwrap_or_default();
            let workspace = group.workspace.clone();
            let collapsed = group.collapsed;
            if retry && !pending_sessions.is_empty() {
                let pending = self.restore_missing_cloud_sessions(index, &pending_sessions);
                if let Some(runtime) = self
                    .cloud_prototype
                    .production
                    .runtimes
                    .get_mut(&self.cloud_prototype.groups.0[index].issue)
                {
                    runtime.pending_session_attachments = pending;
                }
            }
            self.restore_cloud_members(index, members);
            if discovered {
                self.restore_missing_cloud_browsers(index, &pending_browsers, &browsers);
            }
            let Some(ws) = self.board.workspace_id_by_local_id(&workspace) else {
                continue;
            };
            for browser in browsers {
                if let Some(id) = self.board.panel_id_by_local_id(&browser.id) {
                    let member = self.cloud_prototype.groups.0[index].panels.contains(&browser.id);
                    let needs_attach = member
                        && self.board.panel(id).is_some_and(|panel| {
                            panel.kind == PanelKind::Browser
                                && (pending_browsers.contains(&browser.id)
                                    || panel.browser().is_none_or(|state| state.backend() != browser.backend))
                        });
                    if needs_attach
                        && self.restore_cloud_member(index, id, false)
                        && let Some(runtime) = self
                            .cloud_prototype
                            .production
                            .runtimes
                            .get_mut(&self.cloud_prototype.groups.0[index].issue)
                    {
                        runtime.pending_browser_attachments.remove(&browser.id);
                    }
                    if member
                        && let Some(panel) = self
                            .board
                            .panel_mut(id)
                            .filter(|panel| panel.kind == PanelKind::Browser)
                    {
                        panel.visible = browser.visible && !collapsed;
                    }
                    continue;
                }
                if browser.lost {
                    continue;
                }
                let mut options = PanelOptions {
                    kind: PanelKind::Browser,
                    remote_target: browser.remote_target.clone(),
                    visible: browser.visible,
                    local_id: Some(browser.id),
                    command: (!browser.url.is_empty()).then_some(browser.url),
                    position: Some(self.cloud_prototype.groups.0[index].next_position(&self.board)),
                    ..PanelOptions::default()
                };
                if self.prepare_cloud_remote_panel(index, &mut options).is_err() {
                    continue;
                }
                if let Ok(id) = self.board.create_panel(options, ws) {
                    self.cloud_panel_created(index, id);
                    if collapsed {
                        self.cloud_prototype.groups.0[index].set_collapsed(&mut self.board, true);
                    }
                    self.save_cloud_prototype();
                }
            }
        }
    }
    fn restore_missing_cloud_browsers(
        &mut self,
        index: usize,
        pending: &std::collections::HashSet<String>,
        browsers: &[horizon_core::browser::CloudViewState],
    ) {
        for local in pending
            .iter()
            .filter(|id| !browsers.iter().any(|browser| &browser.id == *id))
        {
            if let Some(id) = self.board.panel_id_by_local_id(local)
                && self.restore_cloud_member(index, id, true)
                && let Some(runtime) = self
                    .cloud_prototype
                    .production
                    .runtimes
                    .get_mut(&self.cloud_prototype.groups.0[index].issue)
            {
                runtime.pending_browser_attachments.remove(local);
            }
        }
    }
    fn restore_cloud_members(&mut self, index: usize, members: std::collections::HashSet<String>) {
        for local in members {
            let restored = self
                .board
                .panel_id_by_local_id(&local)
                .is_none_or(|id| self.restore_cloud_member(index, id, false));
            if restored
                && let Some(runtime) = self
                    .cloud_prototype
                    .production
                    .runtimes
                    .get_mut(&self.cloud_prototype.groups.0[index].issue)
            {
                runtime.pending_member_attachments.remove(&local);
            }
        }
    }
    fn restore_cloud_member(&mut self, index: usize, id: horizon_core::PanelId, missing: bool) -> bool {
        let Some(panel) = self.board.panel(id) else {
            return false;
        };
        let workspace_id = panel.workspace_id;
        let mut options = PanelOptions {
            local_id: Some(panel.local_id.clone()),
            device_identity: panel.device_identity().cloned(),
            kind: panel.kind,
            name: Some(panel.title.clone()),
            name_is_custom: Some(panel.name_is_custom()),
            position: Some(panel.layout.position),
            size: Some(panel.layout.size),
            visible: panel.visible,
            command: panel.launch_command.clone(),
            remote_target: panel.browser_remote_target().map(str::to_owned),
            browser_config: Some(horizon_core::browser::BrowserConfig {
                backend: panel.browser_backend().unwrap_or(self.template_config.browser.backend),
                ..self.template_config.browser.clone()
            }),
            transcript_root: self.transcript_root.clone(),
            ..PanelOptions::default()
        };
        options.is_restore = true;
        options.restore_as_disconnected_snapshot = false;
        if !missing && self.prepare_cloud_remote_panel(index, &mut options).is_err() {
            return false;
        }
        options.is_restore = false;
        let restored = if missing {
            Panel::restore_failure(
                id,
                workspace_id,
                options,
                "Remote browser process is no longer available. Create a new browser explicitly.",
            )
        } else {
            Panel::spawn(id, workspace_id, options)
        };
        match restored {
            Ok(new) => {
                if let Some(old) = self.board.panel_mut(id) {
                    old.request_shutdown();
                    *old = new;
                }
                self.panel_render_caches.device_ui_state.remove(&id);
                true
            }
            Err(error) => {
                self.cloud_prototype.error = Some(error.to_string());
                false
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests;
