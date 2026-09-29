//! Explicit opt-in to launch the fixed selection once fresh catalog stock returns.
use super::{Production, provider};
use horizon_core::cloud_runtime::prices::watch::Selection;

pub(super) fn selection(form: &Production) -> Result<Selection, &'static str> {
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

/// The current selection with the price on show now, which a watch never starts
/// above. Without a known price there is no limit to hold it to, so it cannot arm.
pub(super) fn armable(form: &Production) -> Result<(Selection, f64), &'static str> {
    let selection = selection(form)?;
    let quote = form
        .prices
        .list
        .as_ref()
        .and_then(|list| selection.hourly(&list.value.0, &list.value.1))
        .ok_or("The price of this worker is unknown, so a watch has no price to hold it to.")?;
    Ok((selection, quote))
}

/// Arms the watch on the current selection at the price on show now.
pub(super) fn arm(form: &mut Production) {
    let Ok((selection, quote)) = armable(form) else {
        return;
    };
    form.launch.watch_quote = Some(quote);
    form.launch.price_rose = None;
    form.launch.watch = Some(selection);
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
        || super::storage::launch_reason(form).is_some()
    {
        form.launch.watch = None;
        form.launch.watch_quote = None;
        form.launch.price_rose = None;
        return;
    }
    let Some(fetched) = form.prices.fresh_list() else {
        return;
    };
    // A price above the one shown when the watch started waits for a person.
    let (list, preferences) = &fetched.value;
    let hourly = watch.hourly(list, preferences);
    // A worker the catalog no longer prices is no longer a stock wait: the watch ends
    // and the dialog says what is wrong with the selection.
    let (Some(now), Some(quoted)) = (hourly, form.launch.watch_quote) else {
        form.launch.watch = None;
        form.launch.watch_quote = None;
        form.launch.price_rose = None;
        return;
    };
    form.launch.price_rose = (now > quoted + 1e-9).then_some(now);
    if form.launch.price_rose.is_some() {
        return;
    }
    let cpu = form
        .prices
        .size(&watch.profile, (watch.profile.cpu, watch.profile.memory_gb))
        .and_then(Result::ok);
    if watch.available(list, cpu) {
        form.launch.watch = None;
        form.launch.watch_quote = None;
        form.launch.submitted = true;
    }
}

// The shared dialog price fixture is available on Unix, matching creation tests.
#[cfg(all(test, unix))]
mod tests;
