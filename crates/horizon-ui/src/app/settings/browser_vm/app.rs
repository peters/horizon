//! The window-side effects of the Browser VM section: the host bridge runs every request
//! off the UI thread, and the Device panel of a ready machine opens in the active
//! workspace.
use egui::Context;
use horizon_core::{PanelKind, PanelOptions, SshConnection};

use super::{Action, READ_INTERVAL};
use crate::app::HorizonApp;
use crate::app::browser_vm_requests::DeviceRoute;
use crate::app::settings::SettingsTab;

/// The name of the Device panel that the section opens for the one-time sign-in.
const DEVICE_PANEL_NAME: &str = "Browser VM";

impl HorizonApp {
    /// Keeps the run flag of the section current and reads the phase when it is due, before
    /// the section draws. An answer arrives in a later frame.
    pub(in crate::app) fn refresh_browser_vm_settings(&mut self, ctx: &Context) {
        let running = self.browser_create_host.provider_usage.browser_vm.running();
        let visible = self
            .settings
            .as_ref()
            .is_some_and(|editor| editor.active_tab == SettingsTab::BrowserVm);
        let request = self
            .settings
            .as_mut()
            .and_then(|editor| editor.browser_vm.sync(visible, running));
        if running {
            // A run in flight changes the record on its own thread. Frames carry the phase
            // and the run flag back into the section.
            ctx.request_repaint_after(READ_INTERVAL / 2);
        }
        if let Some(request) = request {
            self.queue_settings_browser_vm(request, ctx);
        }
    }

    /// Applies what the section recorded while it drew.
    pub(in crate::app) fn apply_browser_vm_settings(&mut self, ctx: &Context) {
        let actions = self
            .settings
            .as_mut()
            .map(|editor| editor.browser_vm.take_actions())
            .unwrap_or_default();
        for action in actions {
            match action {
                Action::Queue(request) => self.queue_settings_browser_vm(request, ctx),
                Action::OpenDevicePanel => self.open_browser_vm_device_panel(ctx),
            }
        }
    }

    /// Opens the Device panel of the ready machine, which is where a person signs in the one
    /// time. The panel reaches the VNC address of the machine through the SSH route of its
    /// host, so the display is never published openly.
    fn open_browser_vm_device_panel(&mut self, ctx: &Context) {
        let route = self
            .settings
            .as_ref()
            .and_then(|editor| editor.browser_vm.device_route());
        let Some(route) = route else {
            self.show_browser_vm_refusal(
                "The machine is not ready, so its display has no SSH route yet. Refresh the phase.".to_string(),
            );
            return;
        };
        let Some(workspace) = self.board.active_workspace else {
            self.show_browser_vm_refusal(
                "Open a workspace before you open the Device panel of the machine.".to_string(),
            );
            return;
        };
        let identity = self
            .settings
            .as_ref()
            .and_then(|editor| editor.browser_vm.device_identity().map(str::to_string));
        let options = PanelOptions {
            name: Some(DEVICE_PANEL_NAME.to_string()),
            kind: PanelKind::Device,
            command: Some(route.vnc.clone()),
            ssh_connection: Some(device_connection(&route, identity.as_deref())),
            ..PanelOptions::default()
        };
        match self.create_panel_with_options(options, workspace) {
            Ok(panel) => {
                self.reveal_new_panel(ctx, workspace, panel);
                self.mark_runtime_dirty();
            }
            Err(error) => {
                self.show_browser_vm_refusal(format!("The Device panel of the machine could not open: {error}"));
            }
        }
    }

    fn show_browser_vm_refusal(&mut self, text: String) {
        if let Some(editor) = self.settings.as_mut() {
            editor.browser_vm.refuse(text);
        }
    }
}

/// The SSH route of the Device panel. The deploy installs one key for root, so the panel
/// names that key: the person's own `~/.ssh` configuration cannot open the machine.
pub(super) fn device_connection(route: &DeviceRoute, identity: Option<&str>) -> SshConnection {
    SshConnection {
        host: route.host.clone(),
        user: Some(route.user.clone()),
        identity_file: identity.map(str::to_string),
        ..SshConnection::default()
    }
}
