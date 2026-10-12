//! What the setup agent is asked to do: derive the worker image from the CI, report
//! its progress to the dialog, and start from what `.horizon` already holds.
use horizon_core::cloud_runtime::{repository::launch::quick_start, setup::Agent};
use std::path::Path;

/// Where the setup starts from. The dialog chooses `New`, `Update` or `Fix`; a new
/// setup in a folder that already holds `.horizon` files continues from them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::app::cloud_panel::production) enum Mode {
    #[default]
    New,
    /// The working copy holds `.horizon` files that the commit does not.
    Continue,
    /// The commit has settings that load; the CI may need more than they give.
    Update,
    /// The committed settings do not load, with this error.
    Fix(String),
}

impl Mode {
    /// `New` becomes `Continue` when `.horizon` already holds a setup in the working copy.
    pub(super) fn resolve(self, repository: &Path) -> Self {
        if self == Self::New && has_setup_files(repository) {
            Self::Continue
        } else {
            self
        }
    }

    /// The dialog heading for the repository `name`.
    pub(super) fn heading(&self, name: &str) -> String {
        match self {
            Self::New => format!("Set up {name}"),
            Self::Continue => format!("Continue setting up {name}"),
            Self::Update => format!("Update the setup of {name}"),
            Self::Fix(_) => format!("Fix the setup of {name}"),
        }
    }

    /// The commit message for the files the agent wrote.
    pub(super) fn commit_message(&self) -> &'static str {
        match self {
            Self::New | Self::Continue => "Add the Horizon cloud worker setup",
            Self::Update => "Update the Horizon cloud worker setup",
            Self::Fix(_) => "Fix the Horizon cloud worker setup",
        }
    }

    fn instruction(&self) -> Option<String> {
        match self {
            Self::New => None,
            Self::Continue => Some(
                "This folder already has uncommitted files in .horizon from an earlier setup. Start from them: keep what still matches the CI, fix what does not, and list what you changed.".to_owned(),
            ),
            Self::Update => Some(
                "This repository already has a committed .horizon/cloud.yml and worker Dockerfile that load. Update them in place: keep every profile choice (provider, image repository, sizes, storage, siblings and capabilities) and change only what the CI needs now. List each change.".to_owned(),
            ),
            Self::Fix(error) => Some(format!(
                "The committed .horizon/cloud.yml does not load. Horizon reports: {error}. Fix the settings in place, keep every profile choice that is valid, and list each change."
            )),
        }
    }
}

/// Whether the working copy holds a setup in `.horizon`.
pub(super) fn has_setup_files(repository: &Path) -> bool {
    [".horizon/cloud.yml", ".horizon/Dockerfile"]
        .iter()
        .any(|file| repository.join(file).is_file())
}

pub(super) fn prompt(agents: &[Agent], (_repository, status): (&Path, &Path), mode: &Mode) -> String {
    let base = quick_start::IMAGE;
    let progress = PROGRESS.replace("STATUS_FILE", &status.display().to_string());
    let mut text = format!(
        "{PROMPT}\n{progress}\nThe worker base for step 4 is {base}.\n{}",
        super::proof::localci_instruction()
    );
    if let Some(instruction) = mode.instruction() {
        text = format!("{text}\n{instruction}");
    }
    text = format!("{text}\n{PRESERVE}");
    // No machine account yet means no preference, not a profile without agents.
    if agents.is_empty() {
        return text;
    }
    let selection = agents.iter().map(|agent| agent.as_str()).collect::<Vec<_>>().join(", ");
    format!("{text}\nMachine account preference for new profiles: agents [{selection}].")
}

/// Every mode keeps what the person chose before.
const PRESERVE: &str = "Preserve existing repository profile choices unless I explicitly ask to change them.";

/// How the agent reports progress to the dialog and takes the person's messages.
const PROGRESS: &str = "Horizon shows your progress in its New cloud dialog. After each step, rewrite the JSON file STATUS_FILE (write a temporary file beside it, then rename it), for example {\"steps\": [{\"title\": \"Read CI\", \"state\": \"done\", \"detail\": \"dotnet-core.yml, check-translations.yml\"}], \"question\": null, \"summary\": null}. Keep the proof and proof_request entries when you rewrite the file. Use these step titles in this order: Read CI, Find tools, Write Dockerfile, Build image, Run CI in image, Fast rerun. A state is pending, running, done, skipped or failed. A detail is one short line of at most 70 characters that a person reads at a glance, such as the tools and versions found or how many tests passed. Never read or pass my credentials, for example gh auth token, without asking me first through question. Never open an interactive question menu: when you need my answer, ask in plain text, put the same short sentence in question, and set it back to null when I answer. My messages can arrive while you work; follow them. Work fast: ask for Horizon's proof as soon as the Dockerfile and cloud.yml are written, write the Dockerfile so Docker layer and package caches are reused, and keep explanations short. Never build the image or run the CI yourself; Horizon does. At the end, set summary to one line such as \"1,463 tests passed in the image · 4 vCPU · 16 GB\".";

/// The setup agent derives the worker image from the repository's own CI first:
/// CI is the tested recipe that turns a clean Linux machine into one that builds.
const PROMPT: &str = "Prepare this repository for Horizon Cloud Workspaces. Derive the worker image from the repository's own CI and files, so that the restore, build and test commands its Linux CI runs also pass on the worker.
1. Read the CI first: the GitHub Actions workflows in .github/workflows. Horizon proves GitHub Actions jobs only; other CI files such as .gitlab-ci.yml or azure-pipelines.yml can still tell you tools and versions. When the repository has no GitHub Actions workflow with a Linux job that builds or tests, stop and tell me through question that Horizon cannot prove its setup yet. Pick the Linux jobs that restore, build and test. Resolve every tool version to its source: follow env values, matrix values and job outputs back to repository files such as global.json, Directory.Build.props, .nvmrc, .python-version, rust-toolchain.toml or go.mod. Also list tools that steps use but CI never installs because the hosted runner provides them, for example shell: pwsh, jq or a preinstalled SDK. Use localci (https://github.com/peters/localci) when it is installed: localci check, then localci plan --json for those jobs. It resolves matrices, needs, outputs and conditions, and each uses: step it skips is a tool the image must provide.
2. Then read AGENTS.md, the README and any existing Dockerfiles or devcontainer files. Reuse their pinned bases and install steps where they match the CI.
3. Show the plan as a table: each CI step, its tool and version, where the version comes from, and how the image provides it, or why the step is skipped (secrets, publishing, other operating systems, services that need Docker). Then continue unless something is ambiguous.
4. Write .horizon/Dockerfile FROM the worker base named below: Horizon's public Ubuntu 24.04 worker with SSH, Git, tmux, gh, Node 22, the Horizon helpers, Claude, Codex, Chromium and a desktop. Add the toolchains as root and pin their versions. Only when the CI runs in another container or operating system, start from that image instead and copy the Horizon helpers as the worker README describes. Sessions start over SSH and do not inherit ENV, so put each toolchain on PATH with a /etc/horizon-worker/session-env.d/20-REPOSITORY.env file (PATH_PREPEND=DIR and KEY=VALUE lines). Also install the localci release named below, checked against its SHA-256, so agents on the worker run the CI jobs in seconds with localci run --fast.
5. Write .horizon/cloud.yml version 1 with a named default profile: provider: runpod only as the default the schema requires (I choose the provider in New cloud, so add nothing provider-specific such as storage tiers, data centers or GPU types), image (ghcr.io/OWNER/REPOSITORY-worker from the origin remote unless I name another), build {context: .horizon, dockerfile: .horizon/Dockerfile, platform: linux/amd64}, min_cpu and min_memory_gb sized for the build, gpu, and capabilities {agents, browsers, desktop} that the repository needs from what the base provides: desktop for a GUI app, browsers only for web apps or browser tests. Preserve GPU and runtime needs; never silently choose a CPU fallback.
6. Horizon proves the image, not you: do not build it or run the CI yourself. When the Dockerfile and cloud.yml are written, put the proof in the status file as \"proof\": {\"workflow\": \"FILE.yml\", \"jobs\": [\"JOB\", ...], \"event\": \"push\", \"skip\": [\"STEP\", ...]}, with the Linux jobs that restore, build and test, and in skip the steps that throw caches away on purpose (for example isolated package caches) or need secrets. Set \"proof_request\" to 1 and wait. Horizon builds the image, runs horizon-worker-check --git-auth, runs the jobs with localci in a container of the image as UID 10001 (the committed repository at /workspace/checkout, HOME on a volume that stays between runs, your session-env.d applied, this computer's NuGet packages as a read-only fallback folder), runs them again with --fast, and then tells you the result in the terminal. When a step fails, read the log it names, fix the Dockerfile, cloud.yml or the proof, and increase proof_request by one. Horizon owns the Build image, Run CI in image and Fast rerun steps; leave them as they are in the status file. When any profile asks for a GPU, Horizon proves the setup only on a computer with an NVIDIA GPU, and the CI runs with that GPU in the default profile's image. Never change the repository's CI, tests or source to make the proof pass; when a job cannot run in a container, prove with the steps or commands that can and say what was left out.
Keep all credentials in machine-local bindings, never in YAML, source, images or build caches; skip CI steps that need secrets instead of copying them. Do not allocate compute, push images, commit, or open a PR. References: https://github.com/peters/horizon/blob/main/docs/cloud-workspaces.md and https://github.com/peters/horizon/blob/main/examples/cloud-worker/README.md. When you are done, write your summary and ask me to review the setup and click Use this setup in the dialog; do not commit or reload it yourself.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_keeps_the_profile_choices_and_says_where_it_starts() {
        let status = Path::new("/status.json");
        let new = prompt(&[], (Path::new("/repo"), status), &Mode::New);
        assert!(new.contains(PRESERVE), "even without an agent preference");
        assert!(!new.contains(".horizon from an earlier setup"));
        assert!(prompt(&[], (Path::new("/repo"), status), &Mode::Continue).contains("uncommitted files in .horizon"));
        assert!(prompt(&[], (Path::new("/repo"), status), &Mode::Update).contains("Update them in place"));
        let fix = prompt(
            &[],
            (Path::new("/repo"), status),
            &Mode::Fix("profiles.cpu: unknown field `cpus`".into()),
        );
        assert!(fix.contains("Horizon reports: profiles.cpu: unknown field `cpus`."));
    }

    #[test]
    fn a_new_setup_continues_from_files_an_earlier_one_left() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(Mode::New.resolve(temp.path()), Mode::New);
        std::fs::create_dir(temp.path().join(".horizon")).unwrap();
        std::fs::write(temp.path().join(".horizon/Dockerfile"), "FROM scratch\n").unwrap();
        assert_eq!(Mode::New.resolve(temp.path()), Mode::Continue);
        assert_eq!(
            Mode::Update.resolve(temp.path()),
            Mode::Update,
            "an update stays an update"
        );
    }

    #[test]
    fn the_heading_and_commit_name_the_mode() {
        assert_eq!(Mode::Continue.heading("youpayv2"), "Continue setting up youpayv2");
        assert_eq!(
            Mode::Fix(String::new()).heading("youpayv2"),
            "Fix the setup of youpayv2"
        );
        assert_eq!(Mode::Update.commit_message(), "Update the Horizon cloud worker setup");
    }
}
