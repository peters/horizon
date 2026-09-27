//! Explicit opt-in to launch the fixed selection once fresh catalog stock returns.
use super::{Production, can_submit, provider};
use horizon_core::cloud_runtime::prices::watch::Selection;

fn selection(form: &Production) -> Result<Selection, &'static str> {
    if form.launch.loading() {
        return Err("Wait for the repository profile before watching stock.");
    }
    let profile = form
        .profiles
        .as_ref()
        .and_then(|config| config.profiles.get(&form.selected_profile))
        .ok_or("Read the repository profile before watching stock.")?;
    let profile = provider::sized(provider::current(form.provider, profile), profile, form.size)
        .map_err(|_| "Choose a supported size before watching stock.")?;
    Selection::new(profile, form.placement.clone())
}

pub(super) fn controls(ui: &mut egui::Ui, form: &mut Production) {
    ui.add_space(16.0);
    ui.separator();
    ui.add_space(8.0);
    if let Some(watch) = &form.launch.watch {
        ui.label(format!(
            "Watching {} · {} vCPU · {} GB memory. This cloud starts automatically when stock returns.",
            watch.placement.data_centers.join(", "),
            watch.profile.cpu,
            watch.profile.memory_gb,
        ));
        ui.small("Keep this dialog open. Stop watching to edit the selection. Cancel closes the watch.");
        if ui.button("Stop watching").clicked() {
            form.launch.watch = None;
        }
    } else if form.pending_creation.is_none() && !form.launch.submitted {
        let selected = selection(form);
        if ui
            .add_enabled(
                can_submit(form) && selected.is_ok(),
                egui::Button::new("Watch stock & start"),
            )
            .clicked()
        {
            form.launch.watch = selected.as_ref().ok().cloned();
        }
        if let Err(reason) = selected {
            ui.small(reason);
        } else {
            ui.small("Checks every 15 seconds while this dialog stays open. Starts the selected cloud automatically at the current price when stock is available.");
        }
    }
}

pub(super) fn poll(form: &mut Production) {
    let Some(watch) = &form.launch.watch else {
        return;
    };
    if selection(form).as_ref() != Ok(watch)
        || form.title.trim().is_empty()
        || form.pending_creation.is_some()
        || form.launch.submitted
        || form.launch.siblings.blocks_launch()
    {
        form.launch.watch = None;
        return;
    }
    let Some(fetched) = form.prices.fresh_list() else {
        return;
    };
    let cpu = form
        .prices
        .size(&watch.profile, (watch.profile.cpu, watch.profile.memory_gb))
        .and_then(Result::ok);
    if watch.available(&fetched.value.0, cpu) {
        form.launch.watch = None;
        form.launch.submitted = true;
    }
}

// The shared dialog price fixture is available on Unix, matching creation tests.
#[cfg(all(test, unix))]
mod tests;
