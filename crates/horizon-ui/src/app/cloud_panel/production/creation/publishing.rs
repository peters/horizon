//! Local image publishing choices and their background preparation.
mod state;
mod view;
pub(in crate::app::cloud_panel::production) use state::{Key, State};
pub(in crate::app::cloud_panel::production) use view::section;

use super::super::Production;
use horizon_core::cloud_runtime::{self as runtime, publishing::Selection};
use std::path::Path;

const REVIEW: &str = "Review the image destination and worker pull access before starting";
const CHANGED: &str = "The image destination or profile changed; review it before starting";

#[derive(Clone, PartialEq, Eq)]
pub(in crate::app::cloud_panel::production) struct Confirmation {
    scope: Key,
    selection: Selection,
}

pub(in crate::app::cloud_panel::production) fn update(form: &mut Production, root: Option<&Path>, ctx: &egui::Context) {
    // Existing app tests never read the developer's real cloud settings.
    if cfg!(test) {
        return;
    }
    update_for_root(form, root, ctx);
}

fn update_for_root(form: &mut Production, root: Option<&Path>, ctx: &egui::Context) {
    let profile = form
        .profiles
        .as_ref()
        .and_then(|profiles| profiles.profiles.get(&form.selected_profile))
        .filter(|profile| profile.build.is_some());
    let Some(profile) = profile else {
        form.publishing.sync(None, ctx);
        return;
    };
    let fallback;
    let root = if let Some(root) = root {
        root
    } else {
        fallback = horizon_core::HorizonHome::resolve().root().join("cloud");
        &fallback
    };
    if form
        .publishing
        .matches(root, &form.repository, &form.selected_profile, profile)
    {
        form.publishing.poll(ctx);
    } else {
        form.publishing.sync(
            Some(Key {
                root: root.to_owned(),
                raw_repository: form.repository.clone(),
                repository: horizon_core::Config::expand_tilde(&form.repository),
                profile_name: form.selected_profile.clone(),
                profile: profile.clone(),
            }),
            ctx,
        );
    }
}

fn profile(form: &Production) -> Option<&horizon_core::cloud_panel::Profile> {
    form.profiles.as_ref()?.profiles.get(&form.selected_profile)
}

fn current_scope(form: &Production, profile: &horizon_core::cloud_panel::Profile) -> bool {
    form.publishing
        .matches_profile(&form.repository, &form.selected_profile, profile)
}

pub(in crate::app::cloud_panel::production) fn blocked(form: &Production) -> Option<&'static str> {
    profile(form)
        .filter(|profile| profile.build.is_some())
        .filter(|profile| !current_scope(form, profile) || !form.publishing.ready())
        .map(|_| REVIEW)
}

pub(super) fn start_blocked(form: &Production) -> Option<&'static str> {
    profile(form)
        .filter(|profile| profile.build.is_some())
        .filter(|profile| !current_scope(form, profile) || !form.publishing.reviewable())
        .map(|_| REVIEW)
}

pub(super) fn start_confirmation(form: &Production) -> runtime::Result<Option<Confirmation>> {
    if start_blocked(form).is_some() {
        return Err(runtime::Error::Invalid(REVIEW));
    }
    Ok(profile(form)
        .filter(|profile| profile.build.is_some())
        .and_then(|_| form.publishing.reviewed_snapshot()))
}

pub(in crate::app::cloud_panel::production) fn waiting(form: &Production) -> bool {
    profile(form).is_some_and(|profile| {
        profile.build.is_some() && (!current_scope(form, profile) || form.publishing.refreshing())
    })
}

pub(in crate::app::cloud_panel::production) fn confirmation(
    form: &Production,
) -> runtime::Result<Option<Confirmation>> {
    if blocked(form).is_some() {
        return Err(runtime::Error::Invalid(REVIEW));
    }
    Ok(profile(form)
        .filter(|profile| profile.build.is_some())
        .and_then(|_| form.publishing.snapshot()))
}

pub(in crate::app::cloud_panel::production) fn launch_choice(form: &Production) -> runtime::Result<Option<Selection>> {
    let current = confirmation(form)?;
    if let Some(expected) = &form.launch.publishing {
        if current.as_ref() != Some(expected) {
            return Err(runtime::Error::Invalid(CHANGED));
        }
    } else if form.launch.submitted && current.is_some() {
        return Err(runtime::Error::Invalid(REVIEW));
    }
    Ok(current.map(|confirmation| confirmation.selection))
}

fn stop_watch(form: &mut Production, reason: String) {
    form.launch.watch = None;
    form.launch.watch_quote = None;
    form.launch.price_rose = None;
    form.publishing.notice = Some(reason);
}

pub(in crate::app::cloud_panel::production) fn queued_refusal(form: &Production) -> Option<&'static str> {
    blocked(form).or_else(|| {
        let matches = if profile(form).is_some_and(|profile| profile.build.is_some()) {
            form.launch
                .publishing
                .as_ref()
                .is_some_and(|proof| form.publishing.matches_confirmation(proof))
        } else {
            form.launch.publishing.is_none()
        };
        (!matches).then_some(CHANGED)
    })
}

pub(super) fn watch_ready(form: &mut Production) -> bool {
    if waiting(form) {
        return false;
    }
    if let Some(reason) = queued_refusal(form) {
        stop_watch(form, reason.into());
        return false;
    }
    true
}

pub(super) fn show(ui: &mut egui::Ui, form: &mut Production) {
    view::feedback(ui, &mut form.publishing);
    if let Some(info) = &form.publishing.info {
        if !info.enabled {
            if profile(form).is_some_and(horizon_core::cloud_panel::Profile::automatic_image) {
                ui.label("This profile needs an explicit image until portable image publishing is enabled.");
            }
            return;
        }
    } else if form.publishing.error.is_none() && !form.publishing.active() {
        return;
    }
    let editable = form.pending_creation.is_none() && !form.launch.submitted && form.launch.watch.is_none();
    if let Some(target) = ui
        .add_enabled_ui(editable, |ui| section(ui, &mut form.publishing))
        .inner
    {
        form.publishing.request_setup(target);
    }
}

#[cfg(test)]
mod tests;
