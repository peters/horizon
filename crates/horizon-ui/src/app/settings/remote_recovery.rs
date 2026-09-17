use egui::Ui;
use horizon_core::browser::{RemoteRecoveryStatus, remote_recovery::RemoteAllocations};

pub(super) fn render(ui: &mut Ui, allocations: &mut RemoteAllocations) {
    allocations.poll();
    let summaries = allocations.summaries(None);
    if summaries.is_empty() {
        return;
    }
    super::section_heading(ui, "Remote allocations");
    for allocation in summaries {
        super::section_card(ui, |ui| {
            ui.label(format!("{} · {}", allocation.provider, allocation.reference));
            ui.label(&allocation.message);
            let enabled = !matches!(
                allocation.status,
                RemoteRecoveryStatus::InUse
                    | RemoteRecoveryStatus::Reconciling
                    | RemoteRecoveryStatus::Released
                    | RemoteRecoveryStatus::IdentityUnavailable
            );
            if ui.add_enabled(enabled, egui::Button::new("Reconcile")).clicked() {
                let _ = allocations.reconcile(&allocation.reference, None);
            }
            if allocation.status == RemoteRecoveryStatus::Reconciling {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
            }
        });
    }
}
