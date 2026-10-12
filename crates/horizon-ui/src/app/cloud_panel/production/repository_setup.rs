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

mod agent;
mod commit;
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
    if let Some(run) = form
        .setup_run
        .as_mut()
        .filter(|run| same_folder(&run.repository, &repository))
    {
        let name = run.repository.file_name().map_or_else(
            || "the repository".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        // In one column the output well follows the steps, with the height the window leaves.
        let output_height = (!output_beside).then(|| (ui.ctx().content_rect().height() - 700.0).clamp(160.0, 900.0));
        return run::render(ui, run, &name, output_height).map_or(Choice::None, Choice::Run);
    }
    if form.launch.unconfigured {
        return unconfigured(ui, form);
    }
    if let Some(error) = error {
        return fix(ui, form, error);
    }
    ui.collapsing("No cloud configuration yet?", |ui| setup_agent(ui, form, "Set up"))
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
        if setup_agent(ui, form, "Fix with") == Choice::SetupAgent {
            form.setup_mode = Mode::Fix(error.to_owned());
            choice = Choice::SetupAgent;
        }
    });
    choice
}

fn unconfigured(ui: &mut egui::Ui, form: &mut Production) -> Choice {
    let mut choice = Choice::None;
    ui.label(
        RichText::new("No cloud setup yet")
            .size(15.0)
            .strong()
            .color(theme::FG()),
    );
    ui.add_space(6.0);
    // Side by side when both cards fit with their buttons; stacked in a narrow dialog.
    if ui.available_width() >= 520.0 {
        ui.columns(2, |columns| {
            quick_start_card(&mut columns[0], &mut choice);
            setup_card(&mut columns[1], form, &mut choice);
        });
    } else {
        quick_start_card(ui, &mut choice);
        ui.add_space(8.0);
        setup_card(ui, form, &mut choice);
    }
    choice
}

fn quick_start_card(ui: &mut egui::Ui, choice: &mut Choice) {
    option_card(ui, theme::FG_DIM(), |ui| {
        ui.label(RichText::new("Quick start").size(15.0).strong().color(theme::FG()));
        ui.label(RichText::new(QUICK_START_DETAIL).size(12.5).color(theme::FG_SOFT()));
        ui.add_space(8.0);
        if ui
            .add(
                action_button("Start on the base image")
                    .min_size(Vec2::new(120.0, 40.0))
                    .corner_radius(10),
            )
            .clicked()
        {
            *choice = Choice::QuickStart;
        }
    });
}

fn setup_card(ui: &mut egui::Ui, form: &mut Production, choice: &mut Choice) {
    // Files an earlier setup left uncommitted are where the agent continues.
    let earlier = prompt::has_setup_files(Path::new(form.repository.trim()));
    option_card(ui, theme::ACCENT(), |ui| {
        ui.label(RichText::new("Set up from CI").size(15.0).strong().color(theme::FG()));
        let detail = if earlier { CONTINUE_DETAIL } else { SETUP_DETAIL };
        ui.label(RichText::new(detail).size(12.5).color(theme::FG_SOFT()));
        ui.add_space(8.0);
        let verb = if earlier { "Continue with" } else { "Set up with" };
        if setup_agent(ui, form, verb) == Choice::SetupAgent {
            form.setup_mode = Mode::New;
            *choice = Choice::SetupAgent;
        }
    });
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
    let mut footer = Footer::None;
    ui.horizontal(|ui| {
        let hint = if finished {
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
                if ui.add(primary).clicked() {
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
        .is_some_and(|run| same_folder(&run.repository, &form.repository))
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

const QUICK_START_DETAIL: &str = "A 2 vCPU, 4 GB RunPod worker with Claude, Codex, Chromium and a desktop. It needs no registry login, no image build and no Docker on this computer.";
const SETUP_DETAIL: &str = "An agent reads your CI, builds a worker image and runs your tests in it.";
const CONTINUE_DETAIL: &str = "This folder has uncommitted files in .horizon. An agent continues from them, checks them against your CI and runs your tests.";

fn same_folder(a: &Path, b: &str) -> bool {
    let b = Path::new(b.trim());
    a == b || std::fs::canonicalize(b).is_ok_and(|b| a == b)
}

/// The start button, `verb` and the agent's name, with the agent chooser beside it.
/// Why the setup agent is not offered on Windows: Horizon can neither end an agent's
/// processes there nor be sure that nothing it started still writes.
pub(super) const WINDOWS_NOTE: &str =
    "Set up from CI runs on Linux and macOS. On Windows, use Quick start or write .horizon yourself.";

fn setup_agent(ui: &mut egui::Ui, form: &mut Production, verb: &str) -> Choice {
    if cfg!(windows) {
        ui.label(RichText::new(WINDOWS_NOTE).size(12.5).color(theme::FG_SOFT()));
        return Choice::None;
    }
    if busy_elsewhere(ui, form) {
        return Choice::None;
    }
    let presets = setup_choices(form);
    let chosen = chosen_preset(form, &presets).map_or_else(|| "Claude Code".to_owned(), |preset| preset.name.clone());
    let mut open = false;
    ui.horizontal(|ui| {
        let label = format!("{verb} {chosen}");
        let primary = egui::Button::new(RichText::new(label).size(14.0).strong().color(theme::BG()))
            .fill(theme::ACCENT())
            .corner_radius(10)
            .min_size(Vec2::new(120.0, 40.0));
        open = ui.add_enabled(!form.repository.trim().is_empty(), primary).clicked();
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
            .min_size(Vec2::new(0.0, 28.0));
            if ui.add(chip).clicked() {
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

/// Under the profiles: which image a quick start runs and how to leave it, or why
/// quick start does not apply to a commit with its own settings. Committed settings
/// can be updated from the CI; true when the person asked for that.
pub(super) fn quick_start_note(ui: &mut egui::Ui, form: &mut Production) -> bool {
    if let Some(note) = quick_start_text(form) {
        ui.label(RichText::new(note).size(12.0).color(theme::FG_SOFT()));
    }
    if form.profiles.is_none() || form.launch.configuration != Configuration::Committed {
        return false;
    }
    ui.add_space(4.0);
    if busy_elsewhere(ui, form) {
        return false;
    }
    let update = ui
        .add(action_button("Update setup from CI"))
        .on_hover_text("An agent checks the image against your CI now and updates .horizon in place.")
        .clicked();
    if update {
        form.setup_mode = Mode::Update;
    }
    update
}

fn quick_start_text(form: &Production) -> Option<String> {
    form.profiles.as_ref()?;
    match form.launch.configuration {
        Configuration::QuickStart => {
            let (image, digest) = quick_start::IMAGE
                .split_once("@sha256:")
                .unwrap_or((quick_start::IMAGE, ""));
            Some(format!(
                "Quick start on the public base image {image}, digest {}. No registry login, no image build and no Docker on this computer. To use the repository's own settings, choose Read .horizon/cloud.yml in More options.",
                digest.get(..12).unwrap_or(digest)
            ))
        }
        // Profiles read from the commit or the working copy need no note.
        Configuration::Committed | Configuration::LocalImageOnly => None,
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
        let detail = QUICK_START_DETAIL.to_lowercase();
        assert_eq!(profile.provider, "runpod");
        assert!(detail.contains("runpod"));
        assert!(detail.contains(&format!("{} vcpu, {} gb", profile.cpu, profile.memory_gb)));
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
        assert_eq!(quick_start_text(&form), None, "committed profiles need no note");
        form.launch.configuration = Configuration::QuickStart;
        let note = quick_start_text(&form).unwrap();
        assert!(note.starts_with("Quick start on the public base image"), "{note}");
        form.launch.configuration = Configuration::LocalImageOnly;
        assert_eq!(quick_start_text(&form), None, "the working copy is not the commit");
    }

    #[test]
    fn selected_agent_preference_is_passed_without_machine_bindings() {
        let text = prompt(
            &[horizon_core::cloud_runtime::setup::Agent::Claude],
            Path::new("/status.json"),
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
            Path::new("/status.json"),
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
        let text = prompt(&[], Path::new("/status.json"), &Mode::New);
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
        app.start_cloud_repository_setup(&ctx);
        assert!(app.cloud_prototype.error.is_none());
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
}
