//! A repository without cloud settings: quick start on the public base image, or a
//! real local agent prepares the settings, before any allocation.
use super::{PanelKind, Production};
use crate::app::cloud_panel::runtime::action_button;
use crate::theme;
use egui::{Color32, Frame, RichText, Stroke, Vec2};
use horizon_core::{
    cloud_panel::Placement,
    cloud_runtime::repository::launch::{Configuration, quick_start},
};
use std::path::Path;

mod accept;
mod access;
mod agent;
mod bounded;
mod cards;
mod commit;
mod pins;
mod preview;
mod prompt;
mod proof;
mod run;
mod session;
mod snapshot;
mod untrusted;
pub(super) use prompt::Mode;
#[cfg(test)]
use prompt::prompt;
pub(super) use run::{Action, Run};
pub(super) use session::Starting;

/// What the person chose for a repository without cloud settings.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Choice {
    None,
    QuickStart,
    SetupAgent,
    /// Something in the progress view of a running setup agent.
    Run(Action),
}

/// The next steps for a repository without cloud settings. A commit with no
/// `.horizon/cloud.yml` is a normal start, not a fault, so both choices show open as
/// guidance. After another read failure, `error`, the agent is offered to fix the settings.
pub(super) fn render(ui: &mut egui::Ui, form: &mut Production, output_beside: bool, error: Option<&str>) -> Choice {
    if !output_beside {
        ui.add_space(12.0);
    }
    let repository = form.repository.clone();
    if let Some(run) = form.setup_run.as_mut().filter(|run| same_folder(run, &repository)) {
        let name = run.repository.file_name().map_or_else(
            || "the repository".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        // In one column the output well follows the steps, with the height the window leaves.
        let output_height = (!output_beside).then(|| (ui.ctx().content_rect().height() - 700.0).clamp(160.0, 900.0));
        return run::render(ui, run, &name, output_height).map_or(Choice::None, Choice::Run);
    }
    if form.launch.unconfigured {
        return cards::unconfigured(ui, form);
    }
    if let Some(error) = error {
        return fix(ui, form, error);
    }
    ui.collapsing("No cloud configuration yet?", |ui| {
        setup_agent(ui, form, "Set up", false)
    })
    .body_returned
    .unwrap_or(Choice::None)
}

/// Settings that do not load: the agent starts from the error instead of from nothing.
fn fix(ui: &mut egui::Ui, form: &mut Production, error: &str) -> Choice {
    let mut choice = Choice::None;
    option_card(ui, theme::PALETTE_YELLOW(), |ui| {
        ui.label(
            RichText::new("Fix the settings with an agent")
                .size(15.0)
                .strong()
                .color(theme::FG()),
        );
        ui.label(
            RichText::new("It starts from the read error shown below, keeps the profile choices that are valid and proves the result.")
                .size(12.5)
                .color(theme::FG_SOFT()),
        );
        ui.add_space(8.0);
        if setup_agent(ui, form, "Fix with", false) == Choice::SetupAgent {
            form.setup_mode = Mode::Fix(error.to_owned());
            choice = Choice::SetupAgent;
        }
    });
    choice
}

/// The preparation view's heading: no provider yet, because nothing is rented.
pub(super) fn heading(ui: &mut egui::Ui, form: &Production) {
    let name = Path::new(form.repository.trim()).file_name().map_or_else(
        || "the repository".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let title = form
        .setup_run
        .as_ref()
        .map_or_else(|| Mode::New.heading(&name), |run| run.mode.heading(&name));
    ui.label(RichText::new(title).size(26.0).strong().color(theme::FG()));
    ui.label(
        RichText::new("An agent prepares a worker image from your CI. Nothing is rented yet.")
            .size(14.0)
            .color(theme::FG_SOFT()),
    );
}

/// What the preparation view's footer asks for.
pub(super) enum Footer {
    None,
    Close,
    Run(Action),
}

/// Close keeps the agent working; Use this setup appears once it reports a summary.
pub(super) fn footer(ui: &mut egui::Ui, form: &Production) -> Footer {
    let finished = form.setup_run.as_ref().is_some_and(Run::finished);
    let ending = form.setup_run.as_ref().is_some_and(Run::ending);
    let mut footer = Footer::None;
    ui.horizontal(|ui| {
        let hint = if ending {
            "Ending the setup agent before Horizon reads and commits the setup…"
        } else if finished {
            "Use this setup commits the files in .horizon and reads them as this repository's profile."
        } else {
            "Close keeps the agent working. Open New cloud again to come back."
        };
        ui.label(RichText::new(hint).size(13.0).color(theme::FG_SOFT()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if finished {
                let primary = egui::Button::new(RichText::new("Use this setup").size(14.0).strong().color(theme::BG()))
                    .fill(theme::ACCENT())
                    .corner_radius(10)
                    .min_size(Vec2::new(120.0, 40.0));
                if ui.add_enabled(!ending, primary).clicked() {
                    footer = Footer::Run(Action::Use);
                }
            }
            let close = egui::Button::new(RichText::new("Close").size(14.0)).min_size(Vec2::new(120.0, 40.0));
            if ui.add(close).clicked() {
                footer = Footer::Close;
            }
        });
    });
    footer
}

/// Whether a setup agent is working for the repository the dialog shows.
/// The folder of a setup still working for another repository: one setup runs at a time.
fn working_elsewhere(form: &Production) -> Option<String> {
    form.setup_run
        .as_ref()
        .filter(|run| !run.finished() && !run.stopped && !running_here(form))
        .map(|run| run.repository.display().to_string())
}

/// Says so, and offers nothing to start, while a setup still works for another
/// repository: one setup runs at a time.
fn busy_elsewhere(ui: &mut egui::Ui, form: &Production) -> bool {
    let Some(other) = working_elsewhere(form) else {
        return false;
    };
    ui.label(
        RichText::new(format!(
            "The setup of {other} is still working. Open New cloud for it to finish or stop it first."
        ))
        .size(12.5)
        .color(theme::PALETTE_YELLOW()),
    );
    true
}

/// A setup agent for this repository is still working, so its terminal may take keys.
pub(super) fn working_here(form: &Production) -> bool {
    running_here(form)
        && form
            .setup_run
            .as_ref()
            .is_some_and(|run| !run.finished() && !run.stopped)
}

pub(super) fn running_here(form: &Production) -> bool {
    form.setup_run
        .as_ref()
        .is_some_and(|run| same_folder(run, &form.repository))
}

fn option_card(ui: &mut egui::Ui, tint: Color32, add: impl FnOnce(&mut egui::Ui)) {
    Frame::new()
        .fill(theme::blend(theme::PANEL_BG_ALT(), tint, 0.04))
        .stroke(Stroke::new(
            1.0,
            theme::alpha(theme::blend(theme::BORDER_SUBTLE(), tint, 0.3), 220),
        ))
        .corner_radius(10)
        .inner_margin(14)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(136.0);
            // Columns lay out justified; card text and buttons keep their natural width.
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), add);
        });
}

/// Whether `entered`, the repository the person typed, is the run's: as typed when the
/// run started, or its folder. Read without the disk, since the dialog draws it.
fn same_folder(run: &run::Run, entered: &str) -> bool {
    let entered = entered.trim();
    run.entered == entered || run.repository == Path::new(entered)
}

/// Why the setup agent is offered on Linux only: there Horizon starts it in a systemd
/// user scope, which holds every process it starts as its descendant, so Use can end
/// them all before it commits. Elsewhere such a process could leave and keep writing.
pub(super) const PLATFORM_NOTE: &str = "Set up from CI runs on Linux, where Horizon can end every process the agent starts. Elsewhere, use Quick start or write .horizon yourself.";

/// The start button, `verb` and the agent's name, `wide` across its column, with the
/// agent chips below.
fn setup_agent(ui: &mut egui::Ui, form: &mut Production, verb: &str, wide: bool) -> Choice {
    if !cfg!(target_os = "linux") {
        ui.label(RichText::new(PLATFORM_NOTE).size(12.5).color(theme::FG_SOFT()));
        return Choice::None;
    }
    if busy_elsewhere(ui, form) {
        return Choice::None;
    }
    if form.setup_starting.is_some() {
        ui.label(
            RichText::new("Reading the repository before the setup agent starts…")
                .size(12.5)
                .color(theme::FG_SOFT()),
        );
        return Choice::None;
    }
    let presets = setup_choices(form);
    // Without a preset the agent that starts is the chosen kind's, so the label names it.
    let chosen = chosen_preset(form, &presets).map_or_else(
        || form.setup_agent.unwrap_or(PanelKind::Claude).display_name().to_owned(),
        |preset| preset.name.clone(),
    );
    let mut open = false;
    ui.horizontal(|ui| {
        let label = format!("{verb} {chosen}");
        // A long preset name is cut short on one line, and named in full on hover.
        let primary = egui::Button::new(RichText::new(&label).size(14.0).strong().color(theme::BG()))
            .truncate()
            .fill(theme::ACCENT())
            .corner_radius(10)
            .min_size(Vec2::new(
                if wide { ui.available_width().max(120.0) } else { 120.0 },
                40.0,
            ));
        open = ui
            .add_enabled(!form.repository.trim().is_empty(), primary)
            .on_hover_text(&label)
            .clicked();
    });
    // The agent presets of the New Workspace menu, as chips: no popup to hide behind the
    // dialog, and every choice in view.
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        for preset in &presets {
            let selected = preset.name == chosen;
            let fill = if selected {
                theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.22)
            } else {
                theme::PANEL_BG_ALT()
            };
            let chip = egui::Button::new(RichText::new(&preset.name).size(12.5).color(if selected {
                theme::FG()
            } else {
                theme::FG_SOFT()
            }))
            .fill(fill)
            .stroke(Stroke::new(
                1.0,
                if selected {
                    theme::blend(theme::BORDER_SUBTLE(), theme::ACCENT(), 0.5)
                } else {
                    theme::BORDER_SUBTLE()
                },
            ))
            .corner_radius(8)
            .min_size(Vec2::new(0.0, 28.0))
            .truncate();
            if ui.add(chip).on_hover_text(&preset.name).clicked() {
                form.setup_preset = Some(preset.name.clone());
            }
        }
    });
    if open { Choice::SetupAgent } else { Choice::None }
}

/// The agents the setup can run, each once and in its default mode: the agents of
/// the New Workspace menu's presets, or every built-in agent when there are none.
/// Preset variants such as permission modes are left out, so the choice is the agent.
pub(super) fn setup_choices(form: &Production) -> Vec<horizon_core::PresetConfig> {
    // The first preset of each agent in the New Workspace menu is its default mode; it
    // keeps its command and arguments. Later variants (YOLO, Auto) are not offered.
    let mut choices: Vec<horizon_core::PresetConfig> = Vec::new();
    for preset in &form.setup_presets {
        if !choices.iter().any(|choice| choice.kind == preset.kind) {
            choices.push(preset.clone());
        }
    }
    if !choices.is_empty() {
        return choices;
    }
    horizon_core::all_agent_kinds()
        .iter()
        .map(|kind| horizon_core::PresetConfig {
            name: kind.display_name().to_owned(),
            alias: None,
            kind: *kind,
            command: None,
            args: Vec::new(),
            resume: horizon_core::PanelResume::default(),
            ssh_connection: None,
        })
        .collect()
}

/// The chosen preset, else the first one of the machine account's preferred agent,
/// else the first one.
pub(super) fn chosen_preset<'a>(
    form: &Production,
    presets: &'a [horizon_core::PresetConfig],
) -> Option<&'a horizon_core::PresetConfig> {
    form.setup_preset
        .as_deref()
        .and_then(|name| presets.iter().find(|preset| preset.name == name))
        .or_else(|| {
            let preferred = form.setup_agent.unwrap_or(PanelKind::Claude);
            presets.iter().find(|preset| preset.kind == preferred)
        })
        .or_else(|| presets.first())
}

/// What the person asked for under the profiles.
pub(super) enum Note {
    None,
    /// An agent checks the repository's settings against its CI.
    UpdateFromCi,
    /// The repository's own `.horizon/cloud.yml` instead of quick start.
    LeaveQuickStart,
}

/// Under the profiles: which image a quick start runs and how to leave it. The
/// repository's own settings can be updated from the CI instead.
pub(super) fn quick_start_note(ui: &mut egui::Ui, form: &mut Production) -> Note {
    if let Some(note) = quick_start_text(form) {
        ui.label(RichText::new(note).size(12.0).color(theme::FG_SOFT()));
        let leave = ui
            .add(action_button("Use .horizon/cloud.yml"))
            .on_hover_text("Read the repository's own settings, from the working copy on the checked-out commit.")
            .clicked();
        return if leave { Note::LeaveQuickStart } else { Note::None };
    }
    if form.profiles.is_none() || form.launch.configuration != Configuration::WorkingCopy {
        return Note::None;
    }
    ui.add_space(4.0);
    // The setup agent runs on Linux only, so elsewhere its note stands where the action would.
    if !cfg!(target_os = "linux") {
        ui.label(RichText::new(PLATFORM_NOTE).size(12.5).color(theme::FG_SOFT()));
        return Note::None;
    }
    if busy_elsewhere(ui, form) {
        return Note::None;
    }
    let update = ui
        .add(action_button("Update setup from CI"))
        .on_hover_text("An agent checks the image against your CI now and updates .horizon in place.")
        .clicked();
    if update {
        form.setup_mode = Mode::Update;
        return Note::UpdateFromCi;
    }
    Note::None
}

fn quick_start_text(form: &Production) -> Option<String> {
    form.profiles.as_ref()?;
    match form.launch.configuration {
        Configuration::QuickStart => {
            let (image, digest) = quick_start::IMAGE
                .split_once("@sha256:")
                .unwrap_or((quick_start::IMAGE, ""));
            Some(format!(
                "Quick start on the public base image {image}, digest {}. No registry login, no image build and no Docker on this computer.",
                digest.get(..12).unwrap_or(digest)
            ))
        }
        // The repository's own profiles need no note.
        Configuration::Committed | Configuration::WorkingCopy => None,
    }
}

/// Switches where the profiles come from. A profile, size, place or sibling chosen
/// from the previous settings no longer applies.
pub(super) fn set_configuration(form: &mut Production, configuration: Configuration) {
    form.launch.configuration = configuration;
    form.selected_profile.clear();
    form.size = None;
    form.placement = Placement::default();
    form.provider = None;
    form.launch.siblings = super::creation::siblings::State::default();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn each_agent_is_offered_once_with_its_default_preset_command_and_arguments() {
        let preset = |name: &str, kind, args: &[&str]| horizon_core::PresetConfig {
            name: name.into(),
            alias: None,
            kind,
            command: Some(format!("/opt/{name}")),
            args: args.iter().map(|&arg| arg.to_owned()).collect(),
            resume: horizon_core::PanelResume::default(),
            ssh_connection: None,
        };
        let form = Production {
            setup_presets: vec![
                preset("Codex", PanelKind::Codex, &["--no-alt-screen"]),
                preset("Codex (YOLO)", PanelKind::Codex, &["--yolo"]),
                preset("Claude Code", PanelKind::Claude, &["--permission-mode", "auto"]),
            ],
            ..Production::default()
        };
        let choices = setup_choices(&form);
        assert_eq!(choices.len(), 2, "no YOLO variant");
        assert_eq!(choices[0].args, ["--no-alt-screen"]);
        assert_eq!(choices[0].command.as_deref(), Some("/opt/Codex"));
        assert_eq!(choices[1].args, ["--permission-mode", "auto"]);
    }

    #[test]
    fn the_quick_start_detail_describes_the_builtin_profile() {
        let config = quick_start::builtin().unwrap();
        let profile = &config.profiles[quick_start::PROFILE];
        let detail = cards::quick_start_facts().join(" ").to_lowercase();
        assert_eq!(profile.provider, "runpod");
        assert!(detail.contains("runpod"));
        assert!(detail.contains(&format!("{} vcpu · {} gb", profile.cpu, profile.memory_gb)));
        for agent in &profile.capabilities.agents {
            assert!(detail.contains(agent.as_str()), "{agent:?}");
        }
        for browser in &profile.capabilities.browsers {
            assert!(detail.contains(browser.as_str()), "{browser:?}");
        }
        assert_eq!(detail.contains("desktop"), profile.capabilities.desktop);
    }

    #[test]
    fn a_commit_with_its_own_settings_says_why_quick_start_does_not_apply() {
        let mut form = Production::default();
        assert_eq!(quick_start_text(&form), None, "nothing is read yet");
        form.profiles = Some(quick_start::builtin().unwrap());
        assert_eq!(
            quick_start_text(&form),
            None,
            "the repository's own profiles need no note"
        );
        form.launch.configuration = Configuration::QuickStart;
        let note = quick_start_text(&form).unwrap();
        assert!(note.starts_with("Quick start on the public base image"), "{note}");
    }

    #[test]
    fn selected_agent_preference_is_passed_without_machine_bindings() {
        let text = prompt(
            &[horizon_core::cloud_runtime::setup::Agent::Claude],
            (Path::new("/repo"), Path::new("/status.json")),
            &Mode::New,
        );
        assert!(text.contains("agents [claude]"));
        assert!(!text.contains("agents [codex"));
        assert!(text.contains("Preserve existing repository profile choices"));
        assert!(!text.contains("settings.json"));
    }

    #[test]
    fn the_setup_agent_starts_from_ci_and_builds_on_the_pinned_base() {
        let text = prompt(
            &[horizon_core::cloud_runtime::setup::Agent::Claude],
            (Path::new("/repo"), Path::new("/status.json")),
            &Mode::New,
        );
        let ci = text.find(".github/workflows").unwrap();
        assert!(
            ci < text.find("AGENTS.md").unwrap(),
            "CI is read before other repository files"
        );
        assert!(text.contains(&format!("The worker base for step 4 is {}.", quick_start::IMAGE)));
        assert!(text.contains("session-env.d"));
        assert!(text.contains("horizon-worker-check --git-auth"));
        assert!(text.contains("localci plan --json"), "CI is read through localci");
        assert!(
            text.contains("localci run --fast"),
            "the worker iterates with a warm rerun"
        );
        assert!(text.contains("Do not allocate compute, push images, commit, or open a PR."));
        assert!(
            text.contains("rewrite the JSON file /status.json"),
            "progress goes to the run's file"
        );
        for step in run::STEPS {
            assert!(text.contains(step), "{step}");
        }
    }

    #[test]
    fn no_machine_account_states_no_agent_preference() {
        let text = prompt(&[], (Path::new("/repo"), Path::new("/status.json")), &Mode::New);
        assert!(!text.contains("agents []"), "{text}");
        assert!(!text.contains("Machine account preference"));
    }

    #[test]
    fn setup_from_a_cloud_uses_a_separate_local_workspace() {
        let (temp, ctx, mut app) =
            crate::app::test_support::test_app_with_startup(horizon_core::StartupDecision::Ephemeral {
                runtime_state: Box::default(),
            });
        let cloud = app.board.create_workspace("Cloud");
        let local = app.board.workspace(cloud).unwrap().local_id.clone();
        let group =
            horizon_core::cloud_panel::CloudGroup::new(1, "Cloud".into(), local, temp.path().into(), [0.0, 0.0]);
        app.board.cloud_groups.0.push(group.clone());
        app.cloud_prototype.groups.0.push(group);
        app.board.active_workspace = Some(cloud);
        app.cloud_prototype.production.repository = temp.path().display().to_string();
        // A non-process panel exercises placement without invoking a real login in unit tests.
        app.cloud_prototype.production.setup_agent = Some(PanelKind::Usage);
        app.cloud_prototype.production.setup_presets = vec![horizon_core::PresetConfig {
            name: "Test panel".into(),
            kind: PanelKind::Usage,
            alias: None,
            command: None,
            args: Vec::new(),
            resume: horizon_core::PanelResume::default(),
            ssh_connection: None,
        }];
        app.start_cloud_repository_setup(&ctx);
        // Only Linux offers the setup agent: elsewhere it says why and starts nothing.
        if !cfg!(target_os = "linux") {
            assert_eq!(app.cloud_prototype.error.as_deref(), Some(PLATFORM_NOTE));
            assert!(app.cloud_prototype.production.setup_starting.is_none());
            return;
        }
        assert!(app.cloud_prototype.error.is_none());
        // The agent starts once its run's baseline was read on a thread.
        let started = std::time::Instant::now();
        while app.cloud_prototype.production.setup_starting.is_some() {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(20),
                "the baseline was read"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
            app.sync_cloud_repository_setup(&ctx);
        }
        let panel = app
            .board
            .panels
            .iter()
            .find(|panel| panel.title == "Cloud repository setup")
            .unwrap();
        assert_ne!(panel.workspace_id, cloud);
        assert!(!panel.visible, "the dialog shows the agent's progress instead");
        assert!(panel.remote_workspace().is_none());
        let run = app.cloud_prototype.production.setup_run.as_ref().unwrap();
        assert_eq!(run.panel, panel.id);
        assert!(!app.board.cloud_groups.contains_panel(&app.board, panel.id));
        let workspace = panel.workspace_id;
        app.board.active_workspace = Some(cloud);
        assert_eq!(app.board.ensure_local_workspace("Cloud setup"), workspace);
    }

    // The setup agent is offered on Linux only.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_agent_fixes_an_account_problem_in_a_browser_panel_beside_it() {
        let (temp, ctx, mut app) =
            crate::app::test_support::test_app_with_startup(horizon_core::StartupDecision::Ephemeral {
                runtime_state: Box::default(),
            });
        app.cloud_prototype.production.repository = temp.path().display().to_string();
        app.cloud_prototype.production.setup_agent = Some(PanelKind::Usage);
        app.cloud_prototype.production.setup_presets = vec![horizon_core::PresetConfig {
            name: "Test panel".into(),
            kind: PanelKind::Usage,
            alias: None,
            command: None,
            args: Vec::new(),
            resume: horizon_core::PanelResume::default(),
            ssh_connection: None,
        }];
        app.start_cloud_repository_setup(&ctx);
        // The agent starts once its run's baseline was read on a thread.
        let started = std::time::Instant::now();
        while app.cloud_prototype.production.setup_starting.is_some() {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(20),
                "the baseline was read"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
            app.sync_cloud_repository_setup(&ctx);
        }
        let fix = access::Fix::GrantOrganization {
            organization: "fintermobilityas".into(),
        };
        let run = app.cloud_prototype.production.setup_run.as_mut().unwrap();
        run.proof = Some(proof::Proof::failed_on(fix.clone()));
        let agent = run.panel;
        let browsers = |app: &crate::app::HorizonApp| {
            app.board
                .panels
                .iter()
                .filter(|panel| panel.kind == PanelKind::Browser && panel.title == "GitHub access")
                .map(|panel| (panel.id, panel.workspace_id))
                .collect::<Vec<_>>()
        };
        app.cloud_repository_setup_action(&ctx, run::Action::Fix(access::Fixer::Agent));
        let opened = browsers(&app);
        assert_eq!(opened.len(), 1);
        let workspace = app.board.panel(agent).unwrap().workspace_id;
        assert_eq!(opened[0].1, workspace, "the agent's browser tools reach it");
        let run = app.cloud_prototype.production.setup_run.as_ref().unwrap();
        assert_eq!(run.fix_panel, Some(opened[0].0));
        assert!(
            app.board.panel(opened[0].0).unwrap().transient,
            "never saved with the board"
        );
        assert_eq!(run.fixer, Some(access::Fixer::Agent));
        assert!(run.submit_at.is_some(), "the task went to the agent");
        app.cloud_repository_setup_action(&ctx, run::Action::Fix(access::Fixer::Person));
        assert_eq!(browsers(&app), opened, "a second try keeps the panel and its sign-in");
        app.close_panel(opened[0].0);
        app.cloud_repository_setup_action(&ctx, run::Action::ShowFix);
        let reopened = browsers(&app);
        assert_eq!(reopened.len(), 1, "a closed panel opens again");
        assert_ne!(reopened[0].0, opened[0].0);
        assert_eq!(
            app.cloud_prototype.production.setup_run.as_ref().unwrap().fix_panel,
            Some(reopened[0].0)
        );
        app.close_panel(agent);
        app.sync_cloud_repository_setup(&ctx);
        assert!(
            browsers(&app).is_empty(),
            "the panel goes when the agent's panel closes"
        );
        assert_eq!(
            app.cloud_prototype.production.setup_run.as_ref().unwrap().fix_panel,
            None
        );
        app.cloud_repository_setup_action(&ctx, run::Action::Stop);
        assert!(browsers(&app).is_empty(), "the panel goes with the run");
        assert_eq!(fix.url(), horizon_core::cloud_runtime::github::publish::APP_PAGE);
    }
}
