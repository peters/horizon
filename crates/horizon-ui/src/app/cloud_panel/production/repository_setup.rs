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
    let base = quick_start::IMAGE;
    // No machine account yet means no preference, not a profile without agents.
    if agents.is_empty() {
        return format!("{PROMPT}\nThe worker base for step 4 is {base}.");
    }
    let selection = agents.iter().map(|agent| agent.as_str()).collect::<Vec<_>>().join(", ");
    format!(
        "{PROMPT}\nThe worker base for step 4 is {base}.\nMachine account preference for new profiles: agents [{selection}]. Preserve existing repository profile choices unless I explicitly ask to change them."
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

const QUICK_START_DETAIL: &str = "A 2 vCPU, 4 GB RunPod worker with Claude, Codex, Chromium and a desktop. It needs no registry login, no image build and no Docker on this computer.";

fn setup_agent(ui: &mut egui::Ui, form: &mut Production) -> Choice {
    ui.label("Let a local agent read this repository's CI and files, then prepare a worker image that runs its build and tests, and its cloud.yml. Review and commit the files, then return here and reload.");
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

/// Under the profiles: which image a quick start runs and how to leave it, or why
/// quick start does not apply to a commit with its own settings.
pub(super) fn quick_start_note(ui: &mut egui::Ui, form: &Production) {
    if let Some(note) = quick_start_text(form) {
        ui.label(RichText::new(note).size(12.0).color(theme::FG_SOFT()));
    }
}

/// Profiles read from the commit mean it has its own `.horizon/cloud.yml`; a missing
/// one leaves no profiles.
const COMMITTED_NOTE: &str =
    "Quick start does not apply: this commit has its own .horizon/cloud.yml, so its profiles are used.";

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
        Configuration::Committed => Some(COMMITTED_NOTE.to_owned()),
        Configuration::LocalImageOnly => None,
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
                // The workspace now holds the setup agent.
                self.cloud_prototype.production.new_workspace = None;
                self.cloud_prototype.error = None;
                self.reveal_new_panel(ctx, workspace, panel);
            }
            Err(error) => self.cloud_prototype.error = Some(error.to_string()),
        }
    }
}

/// The setup agent derives the worker image from the repository's own CI first:
/// CI is the tested recipe that turns a clean Linux machine into one that builds.
const PROMPT: &str = "Prepare this repository for Horizon Cloud Workspaces. Derive the worker image from the repository's own CI and files, so that the restore, build and test commands its Linux CI runs also pass on the worker.
1. Read the CI first: .github/workflows, and other CI files such as .gitlab-ci.yml or azure-pipelines.yml. Pick the Linux jobs that restore, build and test. Resolve every tool version to its source: follow env values, matrix values and job outputs back to repository files such as global.json, Directory.Build.props, .nvmrc, .python-version, rust-toolchain.toml or go.mod. Also list tools that steps use but CI never installs because the hosted runner provides them, for example shell: pwsh, jq or a preinstalled SDK. For GitHub Actions, use localci (https://github.com/peters/localci) when it is installed: localci check, then localci plan --json for those jobs. It resolves matrices, needs, outputs and conditions, and each uses: step it skips is a tool the image must provide.
2. Then read AGENTS.md, the README and any existing Dockerfiles or devcontainer files. Reuse their pinned bases and install steps where they match the CI.
3. Show the plan as a table: each CI step, its tool and version, where the version comes from, and how the image provides it, or why the step is skipped (secrets, publishing, other operating systems, services that need Docker). Then continue unless something is ambiguous.
4. Write .horizon/Dockerfile FROM the worker base named below: Horizon's public Ubuntu 24.04 worker with SSH, Git, tmux, gh, Node 22, the Horizon helpers, Claude, Codex, Chromium and a desktop. Add the toolchains as root and pin their versions. Only when the CI runs in another container or operating system, start from that image instead and copy the Horizon helpers as the worker README describes. Sessions start over SSH and do not inherit ENV, so put each toolchain on PATH with a /etc/horizon-worker/session-env.d/20-REPOSITORY.env file (PATH_PREPEND=DIR and KEY=VALUE lines). For GitHub Actions, also install the newest localci release (x86_64-unknown-linux-musl), pinned by version and checked against its SHA256SUMS, so agents on the worker run the CI jobs in seconds with localci run --fast.
5. Write .horizon/cloud.yml version 1 with a named default profile: provider: runpod, image (ghcr.io/OWNER/REPOSITORY-worker from the origin remote unless I name another), build {context: .horizon, dockerfile: .horizon/Dockerfile, platform: linux/amd64}, min_cpu and min_memory_gb sized for the build, gpu, and capabilities {agents, browsers, desktop} limited to what the base provides. Preserve GPU and runtime needs; never silently choose a CPU fallback.
6. Prove it when Docker is available: build the image and run horizon-worker-check --git-auth in it. Then, in a container of the image as UID 10001, with the repository at /workspace/checkout, HOME on a writable directory and the session-env.d entries applied, run the CI jobs with localci run -w WORKFLOW JOB (add --event push or --ignore-if when a job condition is false locally), or run their commands directly for other CI systems. Run localci a second time with --fast to show the warm rerun a worker gets. Fix the Dockerfile until the jobs pass. Report each CI step as passed, skipped with the reason, or failed, and both run times.
Keep all credentials in machine-local bindings, never in YAML, source, images or build caches; skip CI steps that need secrets instead of copying them. Do not allocate compute, push images, commit, or open a PR. References: https://github.com/peters/horizon/blob/main/docs/cloud-workspaces.md and https://github.com/peters/horizon/blob/main/examples/cloud-worker/README.md. When you are done, ask me to review and commit the setup, then to return to Cloud > New cloud and reload .horizon/cloud.yml.";

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
    fn a_commit_with_its_own_settings_says_why_quick_start_does_not_apply() {
        let mut form = Production::default();
        assert_eq!(quick_start_text(&form), None, "nothing is read yet");
        form.profiles = Some(quick_start::builtin().unwrap());
        assert_eq!(quick_start_text(&form).as_deref(), Some(COMMITTED_NOTE));
        form.launch.configuration = Configuration::QuickStart;
        let note = quick_start_text(&form).unwrap();
        assert!(note.starts_with("Quick start on the public base image"), "{note}");
        form.launch.configuration = Configuration::LocalImageOnly;
        assert_eq!(quick_start_text(&form), None, "the working copy is not the commit");
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
    fn the_setup_agent_starts_from_ci_and_builds_on_the_pinned_base() {
        let text = prompt(&[horizon_core::cloud_runtime::setup::Agent::Claude]);
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
    }

    #[test]
    fn no_machine_account_states_no_agent_preference() {
        let text = prompt(&[]);
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
        assert!(panel.remote_workspace().is_none());
        assert!(!app.board.cloud_groups.contains_panel(&app.board, panel.id));
        let workspace = panel.workspace_id;
        app.board.active_workspace = Some(cloud);
        assert_eq!(app.board.ensure_local_workspace("Cloud setup"), workspace);
    }
}
