//! A repository without cloud settings: quick start on the public base image, or a
//! real local agent prepares the settings, before any allocation.
use super::{HorizonApp, PanelKind, PanelOptions, Production};
use crate::app::cloud_panel::runtime::action_button;
use crate::theme;
use egui::{Frame, RichText, Stroke, Vec2};
use horizon_core::{
    cloud_panel::Placement,
    cloud_runtime::repository::launch::{Configuration, quick_start},
};

fn prompt(agents: &[horizon_core::cloud_runtime::setup::Agent]) -> String {
    let selection = agents.iter().map(|agent| agent.as_str()).collect::<Vec<_>>().join(", ");
    format!(
        "{PROMPT}\nMachine account preference for new profiles: agents [{selection}]. Preserve existing repository profile choices unless I explicitly ask to change them."
    )
}

/// What the person chose for a repository without cloud settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Choice {
    None,
    QuickStart,
    SetupAgent,
}

/// The next steps for a repository without cloud settings. A commit with no
/// `.horizon/cloud.yml` is a normal start, not a fault, so both choices show open as
/// guidance; after another read failure the setup agent stays behind a section.
pub(super) fn render(ui: &mut egui::Ui, form: &mut Production) -> Choice {
    ui.add_space(12.0);
    if form.launch.unconfigured {
        return unconfigured(ui, form);
    }
    ui.collapsing("No cloud configuration yet?", |ui| setup_agent(ui, form))
        .body_returned
        .unwrap_or(Choice::None)
}

fn unconfigured(ui: &mut egui::Ui, form: &mut Production) -> Choice {
    let mut choice = Choice::None;
    Frame::new()
        .fill(theme::PANEL_BG_ALT())
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
        .corner_radius(10)
        .inner_margin(14)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("This commit has no .horizon/cloud.yml")
                    .size(15.0)
                    .strong()
                    .color(theme::FG()),
            );
            ui.label(
                RichText::new("Choose how this repository starts in the cloud.")
                    .size(13.0)
                    .color(theme::FG_SOFT()),
            );
            ui.add_space(10.0);
            if ui
                .add(action_button("Quick start on the public base image").min_size(Vec2::new(0.0, 34.0)))
                .clicked()
            {
                choice = Choice::QuickStart;
            }
            ui.label(RichText::new(QUICK_START_DETAIL).size(12.0).color(theme::FG_SOFT()));
            ui.add_space(14.0);
            ui.label(
                RichText::new("Or prepare the repository's own settings")
                    .size(13.0)
                    .strong()
                    .color(theme::FG()),
            );
            if setup_agent(ui, form) == Choice::SetupAgent {
                choice = Choice::SetupAgent;
            }
        });
    choice
}

const QUICK_START_DETAIL: &str = "A 2 vCPU, 4 GB RunPod worker with Claude, Codex, Chromium and a desktop. It needs no registry login and no image build.";

fn setup_agent(ui: &mut egui::Ui, form: &mut Production) -> Choice {
    ui.label("Let a local agent inspect this repository and prepare its worker image and cloud.yml. Review and commit the files, then return here and reload.");
    ui.small(
        "This setup terminal uses your local agent login. Cloud settings configure authentication on remote workers.",
    );
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(&mut form.setup_agent, Some(PanelKind::Codex), "Codex");
        ui.selectable_value(&mut form.setup_agent, Some(PanelKind::Claude), "Claude");
    });
    let open = ui
        .add_enabled(
            !form.repository.trim().is_empty() && form.setup_agent.is_some(),
            egui::Button::new("Open setup agent").fill(theme::PANEL_BG_ALT()),
        )
        .clicked();
    if open { Choice::SetupAgent } else { Choice::None }
}

/// Under the profile of a quick start: which image runs and how to leave it.
pub(super) fn quick_start_note(ui: &mut egui::Ui, form: &Production) {
    if form.launch.configuration != Configuration::QuickStart || form.profiles.is_none() {
        return;
    }
    let (image, digest) = quick_start::IMAGE
        .split_once("@sha256:")
        .unwrap_or((quick_start::IMAGE, ""));
    ui.label(
        RichText::new(format!(
            "Quick start on the public base image {image}, digest {}. No registry login and no image build. To use the repository's own settings, choose Read .horizon/cloud.yml in More options.",
            digest.get(..12).unwrap_or(digest)
        ))
        .size(12.0)
        .color(theme::FG_SOFT()),
    );
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

impl HorizonApp {
    pub(super) fn start_cloud_repository_setup(&mut self, ctx: &egui::Context) {
        let form = &self.cloud_prototype.production;
        let Some(kind) = form.setup_agent else { return };
        let repository = match std::fs::canonicalize(&form.repository) {
            Ok(path) if path.is_dir() => path,
            _ => {
                self.cloud_prototype.error = Some("Choose an existing local repository directory".into());
                return;
            }
        };
        let workspace = self.board.ensure_local_workspace("Cloud setup");
        // Deliberately local: config preparation must not inherit a focused cloud.
        let result = self.board.create_panel(
            PanelOptions {
                name: Some("Cloud repository setup".into()),
                name_is_custom: Some(true),
                kind,
                cwd: Some(repository),
                args: vec![prompt(&form.setup_agents)],
                transcript_root: self.transcript_root.clone(),
                ..PanelOptions::default()
            },
            workspace,
        );
        match result {
            Ok(panel) => {
                self.cloud_prototype.production.creating = false;
                self.cloud_prototype.error = None;
                self.reveal_new_panel(ctx, workspace, panel);
            }
            Err(error) => self.cloud_prototype.error = Some(error.to_string()),
        }
    }
}

const PROMPT: &str = "Prepare this repository for Horizon Cloud Workspaces. First inspect AGENTS.md, its build/test requirements and existing Dockerfiles. Explain your proposed setup before editing. Create or update .horizon/cloud.yml version 1 with a named default profile, provider: runpod, image, optional build {context, dockerfile, platform: linux/amd64}, min_cpu, min_memory_gb (resource minimums), gpu and explicit capabilities {agents, browsers, desktop}. Preserve required GPU/runtime dependencies; never silently choose a CPU fallback. Ask which coding agents and tools are wanted if not known. Native desktop/VNC does not need browsers. A worker image must satisfy Horizon's SSH, Git, tmux and enabled agent/tool contract; use the documented Horizon worker examples, not an arbitrary application image. Keep all credentials in machine-local bindings, never YAML, source, images or build caches. Do not allocate compute, push images, commit, or open a PR automatically. Run applicable config/build checks, explain prerequisites and ask me to review and commit the setup. Then tell me to return to Cloud > New cloud and reload .horizon/cloud.yml.";

#[cfg(test)]
mod tests {
    use super::*;
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
    fn selected_agent_preference_is_passed_without_machine_bindings() {
        let text = prompt(&[horizon_core::cloud_runtime::setup::Agent::Claude]);
        assert!(text.contains("agents [claude]"));
        assert!(!text.contains("agents [codex"));
        assert!(text.contains("Preserve existing repository profile choices"));
        assert!(!text.contains("settings.json"));
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
        assert!(panel.remote_workspace().is_none());
        assert!(!app.board.cloud_groups.contains_panel(&app.board, panel.id));
        let workspace = panel.workspace_id;
        app.board.active_workspace = Some(cloud);
        assert_eq!(app.board.ensure_local_workspace("Cloud setup"), workspace);
    }
}
