//! Checkbox choices express access intent; the controller reports verified availability.
use super::{Action, Entry};
use horizon_core::cloud_runtime::companions::{Row, Status};

pub(super) fn render(ui: &mut egui::Ui, entry: &mut Entry) {
    ui.separator();
    ui.label("Companion clouds");
    ui.small("Share access to selected clouds. Stopped clouds stay stopped.");
    let busy = entry.job.is_some() || entry.pending.is_some() || !entry.clearing.is_empty();
    if busy {
        ui.spinner();
    } else if !entry.blocked && ui.small_button("Refresh").clicked() {
        entry.queue(Action::Refresh);
    }
    if let Some(error) = &entry.error {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
    let Some(snapshot) = &entry.snapshot else {
        if !busy && entry.error.is_none() {
            ui.small("Declare companion repositories in .horizon/cloud.yml to select them here.");
        }
        return;
    };
    if let Some(notice) = &snapshot.notice {
        ui.small(notice);
    }
    if let Some(error) = &snapshot.publication_error {
        ui.small(format!("Agent discovery update pending: {error}"));
    }
    let stale = entry.error.is_some()
        || now() < snapshot.catalog.observed_at
        || snapshot.catalog.observed_at.saturating_add(60) < now();
    let mut requested = None;
    for row in &snapshot.rows {
        let companion = &row.companion;
        let choice = entry.choices.entry(companion.alias.clone()).or_default();
        ui.push_id(&companion.alias, |ui| {
            let clearing = entry.clearing.contains(&companion.alias);
            let selecting = entry.selecting.contains(&companion.alias);
            if let Some(action) = render_row(ui, row, choice, busy, stale || clearing || selecting, selecting) {
                requested = Some(action);
            }
            if clearing {
                ui.small("Removing access…");
            } else if selecting {
                ui.small("Selection pending · uncheck to cancel");
            }
        });
    }
    if let Some(action) = requested {
        entry.queue(action);
    }
}

pub(super) fn render_row(
    ui: &mut egui::Ui,
    row: &Row,
    choice: &mut String,
    busy: bool,
    stale: bool,
    selecting: bool,
) -> Option<Action> {
    let companion = &row.companion;
    let mut action = None;
    let target = if row.candidates.len() == 1 {
        row.candidates.first().map(|target| target.cloud_id.as_str())
    } else {
        row.candidates
            .iter()
            .find(|target| target.cloud_id == *choice)
            .map(|target| target.cloud_id.as_str())
    };
    let mut selected = companion.selected || selecting;
    let enabled = selected || (!busy && target.is_some() && companion.target_cloud_id.is_none() && !stale);
    let checkbox = ui.add_enabled(enabled, egui::Checkbox::new(&mut selected, &companion.alias));
    if checkbox.changed() {
        action = if selected {
            target.map(|id| Action::Select {
                alias: companion.alias.clone(),
                target_cloud_id: id.into(),
            })
        } else {
            Some(Action::Clear {
                alias: companion.alias.clone(),
            })
        };
    }
    ui.small(format!("{} · {}", companion.repository, companion.profile));
    if let Some(id) = &companion.target_cloud_id {
        ui.small(format!("Cloud {id}"));
    }
    if row.candidates.len() > 1 && !companion.selected && companion.target_cloud_id.is_none() {
        ui.add_enabled_ui(!busy && !stale, |ui| {
            egui::ComboBox::from_id_salt("companion-target")
                .selected_text(if choice.is_empty() {
                    "Choose cloud"
                } else {
                    choice.as_str()
                })
                .show_ui(ui, |ui| {
                    for target in &row.candidates {
                        ui.selectable_value(choice, target.cloud_id.clone(), &target.cloud_id);
                    }
                });
        });
    }
    ui.small(if stale {
        "Status unavailable; waiting for refresh"
    } else {
        status(companion.status)
    });
    if let Some(error) = &row.error {
        ui.small(error);
    }
    if companion.status == Status::Ready
        && !stale
        && let Some(access) = &companion.access
    {
        ui.monospace(format!("ssh {}", access.ssh_alias));
    }
    action
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| time.as_secs())
}

fn status(status: Status) -> &'static str {
    match status {
        Status::Unselected => "Not selected",
        Status::Missing => "Matching cloud is missing",
        Status::Ambiguous => "Choose a matching cloud",
        Status::Stopped => "Selected · stopped",
        Status::Unavailable => "Selected · source is unavailable",
        Status::Connecting => "Checking SSH access…",
        Status::Ready => "Ready · SSH verified",
        Status::Unverified => "Access needs verification",
        Status::Unreachable => "SSH is unreachable",
        Status::Changed => "Identity changed · clear and select again",
        Status::RevocationPending => "Access removal pending · original worker must be reachable",
    }
}
