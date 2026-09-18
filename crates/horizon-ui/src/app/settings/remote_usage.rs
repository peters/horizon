//! Presentation of shared capacity; never participates in session admission.
use std::collections::BTreeMap;
use std::time::Duration;

use egui::Ui;
use horizon_core::browser::remote::RemoteProviderProfile;
use horizon_core::browser::remote_usage::ProviderUsageMonitor;
use horizon_core::remote_browser_credential::CredentialWorkbench;

pub(super) type UsagePanels = BTreeMap<String, ProviderUsageMonitor>;

pub(super) fn render(
    ui: &mut Ui,
    name: &str,
    profile: &RemoteProviderProfile,
    credentials: &CredentialWorkbench,
    panels: &mut UsagePanels,
) {
    if !ProviderUsageMonitor::supported(profile) {
        panels.remove(name);
        return;
    }
    let monitor = panels.entry(name.to_string()).or_default();
    monitor.update(profile, credentials, false);
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        ui.label("Shared provider usage");
        if ui
            .add_enabled(!monitor.refreshing(), egui::Button::new("Refresh").small())
            .clicked()
        {
            monitor.update(profile, credentials, true);
        }
    });
    if let Some((usage, checked_at)) = monitor.sample {
        ui.label(format!(
            "{} / {} sessions running · {} queued",
            usage.running, usage.allowed, usage.queued
        ));
        let age = checked_at.elapsed().as_secs();
        let stale = monitor.error.is_some() || age >= 60;
        super::dim_label(
            ui,
            &format!("{}Updated {age}s ago", if stale { "Stale · " } else { "" }),
        );
    } else if !monitor.refreshing() && monitor.error.is_none() {
        super::dim_label(ui, "Usage has not been checked yet.");
    }
    if monitor.refreshing() {
        super::dim_label(ui, "Refreshing shared usage…");
    }
    if let Some(error) = monitor.error {
        super::dim_label(ui, &error.to_string());
    }
    super::dim_label(
        ui,
        "Shared across clients using this provider account. Availability can change before a session starts.",
    );
    ui.ctx().request_repaint_after(if monitor.refreshing() {
        Duration::from_millis(100)
    } else {
        monitor.next_refresh_in()
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::browser::remote::{
        ControlEndpoint, RemoteAdapterKind, RemoteAuthentication, RemoteSessionLimits,
    };
    use horizon_core::browser::remote_usage::ProviderUsage;
    use horizon_core::remote_browser_credential::FakeCredentialStore;

    #[test]
    fn switching_to_an_unsupported_adapter_discards_cached_usage() {
        let profile = RemoteProviderProfile {
            adapter: RemoteAdapterKind::Webdriver,
            endpoint: ControlEndpoint::parse("https://grid.example.test").expect("endpoint"),
            authentication: RemoteAuthentication::None {},
            credential_bindings: BTreeMap::new(),
            limits: RemoteSessionLimits::default(),
        };
        let mut monitor = ProviderUsageMonitor::default();
        monitor.sample = Some((
            ProviderUsage {
                running: 2,
                allowed: 4,
                queued: 0,
            },
            std::time::Instant::now(),
        ));
        let mut panels = UsagePanels::from([("cloud".into(), monitor)]);
        let credentials = CredentialWorkbench::with_opener(Box::new(|| Ok(Box::new(FakeCredentialStore::new()))));
        let context = egui::Context::default();
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            render(ui, "cloud", &profile, &credentials, &mut panels);
        });
        output.textures_delta.clear();
        assert!(
            !panels.contains_key("cloud"),
            "re-enabling usage must start with a fresh monitor"
        );
    }
}
