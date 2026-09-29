//! Everything Start depends on, listed and confirmed before Start is offered: the repository
//! can be read, the provider accepts this machine's account, the SSH key is usable.
//! Deployment makes the same checks first; here they fail while they can still be fixed.
mod account;

use super::{provider, selector};
use crate::{
    app::cloud_panel::production::{Production, creation::source},
    theme,
};
use egui::{Button, Context, RichText, Sense, Ui, Vec2};
use horizon_core::cloud_runtime::{
    deployment::{self, Problem},
    prices::Profile,
    provider::Kind,
    settings::Settings,
};
use std::{
    path::Path,
    sync::mpsc::{Receiver, channel},
};

/// The profile and provider the last answer was for, so changing either asks again.
type Key = (String, &'static str, String);

#[derive(Default)]
pub(in crate::app::cloud_panel::production) struct State {
    key: Option<Key>,
    job: Option<Receiver<Vec<Problem>>>,
    problems: Option<Vec<Problem>>,
    account: account::State,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Row {
    Pass(String),
    Fail(String),
    Pending(String),
}

impl State {
    /// Starts a check when the chosen profile or provider changed since the last one.
    fn update(&mut self, root: Option<&Path>, chosen: Option<(Key, Profile)>, ctx: &Context) {
        if let Some(problems) = self.job.as_ref().and_then(|job| job.try_recv().ok()) {
            self.problems = Some(problems);
            self.job = None;
        }
        let (Some(root), Some((key, profile))) = (root, chosen) else {
            return;
        };
        if self.key.as_ref() == Some(&key) {
            return;
        }
        // The tab offered for a key is the account this profile runs on.
        self.account.prefer(key.1 == "hetzner");
        self.key = Some(key);
        self.problems = None;
        let (sender, receiver) = channel();
        let (path, ctx) = (root.join("settings.json"), ctx.clone());
        std::thread::spawn(move || {
            let problems = if path.exists() {
                match Settings::load(&path) {
                    Ok(settings) => deployment::admission_problems(&profile, &settings),
                    Err(error) => vec![problem(
                        "Cloud settings",
                        format!("{error}. Open Cloud settings to repair them."),
                    )],
                }
            } else {
                vec![problem(
                    "Cloud settings",
                    "No RunPod or Hetzner key on this computer yet. Paste one below; Horizon makes the SSH key itself."
                        .into(),
                )]
            };
            let _ = sender.send(problems);
            ctx.request_repaint();
        });
        self.job = Some(receiver);
    }
}

fn problem(what: &'static str, reason: String) -> Problem {
    Problem { what, reason }
}

/// Keeps the checks current for the chosen profile; called once per frame with the dialog.
pub(super) fn update(form: &mut Production, root: Option<&Path>, ctx: &Context) {
    if let Some(saved) = form.checks.account.poll() {
        match saved {
            Ok(()) => {
                // The key just saved is asked about again, by the checks and by the provider.
                form.checks.key = None;
                form.prices.refresh();
            }
            Err(error) => form.checks.account.fail(error),
        }
    }
    // UI tests resolve the developer's real Horizon home; they never look at its settings.
    if cfg!(test) || !ready(form) {
        return;
    }
    let chosen = form.profiles.as_ref().and_then(|config| {
        let profile = config.profiles.get(&form.selected_profile)?;
        Some((
            (
                form.selected_profile.clone(),
                provider::current(form.provider, profile).id,
                form.repository.clone(),
            ),
            profile.clone(),
        ))
    });
    let root = settings_root(root);
    form.checks.update(Some(&root), chosen, ctx);
}

/// The folder the machine's cloud settings live in, whether or not the app has named it yet.
fn settings_root(root: Option<&Path>) -> std::borrow::Cow<'_, Path> {
    root.map_or_else(
        || std::borrow::Cow::Owned(horizon_core::HorizonHome::resolve().root().join("cloud")),
        std::borrow::Cow::Borrowed,
    )
}

/// The repository is read and its profile known: the point where the rest of the dialog shows.
fn ready(form: &Production) -> bool {
    form.profiles.is_some() && !form.launch.loading()
}

fn rows(form: &Production) -> Vec<Row> {
    let mut rows = vec![Row::Pass(format!(
        "Repository read: {}",
        horizon_core::dir_search::abbreviate_home(Path::new(&form.repository))
    ))];
    let found = form.checks.problems.as_deref();
    let problem = |what: &str| found.and_then(|found| found.iter().find(|problem| problem.what == what));
    if let Some(problem) = problem("Cloud settings") {
        rows.push(Row::Fail(problem.reason.clone()));
        return rows;
    }
    let Some(profile) = form
        .profiles
        .as_ref()
        .and_then(|config| config.profiles.get(&form.selected_profile))
    else {
        return rows;
    };
    let provider = provider::current(form.provider, profile);
    let (live, error) = match provider.kind {
        Kind::RunPod => (form.prices.list.is_some(), form.prices.list_error.as_deref()),
        Kind::Hetzner => (
            form.prices
                .hetzner
                .fresh()
                .is_some_and(|fetched| fetched.value.is_some()),
            form.prices.hetzner.error(),
        ),
    };
    rows.push(if let Some(problem) = problem("Provider account") {
        Row::Fail(problem.reason.clone())
    } else if let Some(error) = error {
        Row::Fail(error.to_owned())
    } else if live && found.is_some() {
        Row::Pass(format!("{} account accepted, prices are current", provider.label))
    } else {
        Row::Pending(format!("Checking the {} account", provider.label))
    });
    rows.push(match (problem("SSH key"), found) {
        (Some(problem), _) => Row::Fail(format!("SSH key: {}", problem.reason)),
        (None, Some(_)) => Row::Pass("SSH key is usable".into()),
        (None, None) => Row::Pending("Checking the SSH key".into()),
    });
    rows
}

/// The provider account is missing or was turned down, which a key pasted here can fix.
fn needs_account(form: &Production) -> bool {
    let missing = form.checks.problems.as_deref().is_some_and(|found| {
        found
            .iter()
            .any(|problem| matches!(problem.what, "Cloud settings" | "Provider account"))
    });
    missing || provider_error(form).is_some()
}

fn provider_error(form: &Production) -> Option<&str> {
    let profile = form.profiles.as_ref()?.profiles.get(&form.selected_profile)?;
    match provider::current(form.provider, profile).kind {
        Kind::RunPod => form.prices.list_error.as_deref(),
        Kind::Hetzner => form.prices.hetzner.error(),
    }
}

fn unfinished(rows: &[Row]) -> bool {
    rows.iter().any(|row| matches!(row, Row::Fail(_) | Row::Pending(_)))
}

/// Why Start must wait for the checks, once the repository is read.
pub(super) fn blocked(form: &Production) -> Option<&'static str> {
    blocked_given(form, !cfg!(test))
}

fn blocked_given(form: &Production, checking: bool) -> Option<&'static str> {
    (checking && ready(form) && unfinished(&rows(form))).then_some("Start unlocks when every check above passes.")
}

/// The footer: Continue while the repository is still being chosen, Start cloud after.
pub(super) fn footer(ui: &mut Ui, form: &mut Production, actions: &mut super::Actions) {
    if source_step(form) {
        source::continue_footer(ui, form, actions);
    } else {
        selector::summary::footer(ui, form, actions);
    }
}

/// Whether the dialog is still asking where the code is.
pub(super) fn source_step(form: &Production) -> bool {
    form.repository.trim().is_empty() || form.source.editing()
}

/// The summary with the checks under it, or above it while a check stands in the way.
pub(super) fn with_summary(ui: &mut Ui, form: &mut Production, root: Option<&Path>) {
    let failing = !cfg!(test) && ready(form) && rows(form).iter().any(|row| matches!(row, Row::Fail(_)));
    if failing {
        show(ui, form, root);
        ui.add_space(14.0);
    }
    selector::summary::show(ui, form);
    if !failing && !cfg!(test) {
        ui.add_space(14.0);
        show(ui, form, root);
    }
}

fn show(ui: &mut Ui, form: &mut Production, root: Option<&Path>) {
    if !ready(form) {
        return;
    }
    let rows = rows(form);
    let failed = rows.iter().any(|row| matches!(row, Row::Fail(_)));
    selector::widgets::caption(ui, "BEFORE YOU START");
    for row in &rows {
        let (color, text) = match row {
            Row::Pass(text) => (theme::PALETTE_GREEN(), text),
            Row::Fail(text) => (theme::PALETTE_RED(), text),
            Row::Pending(text) => (theme::FG_DIM(), text),
        };
        ui.horizontal_wrapped(|ui| {
            let (dot, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
            ui.painter().circle_filled(dot.center(), 4.0, color);
            let text_color = if failed && matches!(row, Row::Fail(_)) {
                color
            } else {
                theme::FG_SOFT()
            };
            ui.label(RichText::new(text).size(13.0).color(text_color));
        });
    }
    if failed
        && ui
            .add(
                Button::new(RichText::new("Check again").size(13.0))
                    .small()
                    .frame(false),
            )
            .clicked()
    {
        form.checks.key = None;
        form.prices.refresh();
    }
    if needs_account(form) {
        account::form(ui, &mut form.checks.account, &settings_root(root));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form_with(problems: Vec<Problem>) -> Production {
        Production {
            repository: "/work/demo-atlas".into(),
            checks: State {
                problems: Some(problems),
                ..State::default()
            },
            ..Production::default()
        }
    }

    #[test]
    fn without_a_profile_there_is_only_the_repository_row() {
        let rows = rows(&form_with(Vec::new()));
        assert_eq!(rows.len(), 1);
        assert!(matches!(&rows[0], Row::Pass(text) if text.contains("demo-atlas")));
    }

    #[test]
    fn missing_settings_end_the_list_with_the_way_to_fix_them() {
        let rows = rows(&form_with(vec![problem("Cloud settings", "No key yet.".into())]));
        assert_eq!(rows.last(), Some(&Row::Fail("No key yet.".into())));
        assert!(unfinished(&rows));
    }

    #[test]
    fn start_waits_for_the_checks_only_once_the_repository_is_read() {
        let mut form = form_with(Vec::new());
        assert_eq!(blocked_given(&form, true), None, "no profile yet, so nothing to start");
        form.profiles = Some(
            horizon_core::cloud_panel::CloudConfig::parse(
                "version: 1\ndefault: cpu\nprofiles:\n  cpu:\n    provider: runpod\n    image: ghcr.io/demo-org/dev-image\n    cpu: 4\n    memory_gb: 16\n    gpu: false\n    storage:\n      container_gb: 20\n      volume_gb: 60\n",
            )
            .unwrap(),
        );
        form.selected_profile = "cpu".into();
        assert!(blocked_given(&form, true).is_some(), "the account is not confirmed yet");
        assert_eq!(blocked_given(&form, false), None);
    }

    #[test]
    fn the_source_step_is_over_once_a_repository_is_chosen() {
        let mut form = Production::default();
        assert!(source_step(&form));
        form.repository = "/work/demo-atlas".into();
        assert!(!source_step(&form));
    }
}
