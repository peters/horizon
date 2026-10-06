# Epic: Easy first run

Status: proposed, 2026-10-06. This plan uses ASD-STE100 Simplified Technical
English. See [the STE rules](../style/ste-rules.md).

## Purpose

A new user must get from zero to a useful Horizon board in a short time. The
user must not need to be a programmer. An agent must be able to do the full
setup for the user on Linux, macOS and Windows.

This epic also shows new users what Horizon can do. Most users do not know that
an agent can control a browser, a VNC desktop, an iOS Simulator or a cloud
worker from one board.

## Current state (audit of `origin/main` at `7c31b9c91`)

### Install and build

- Release binaries use the default features only (`.github/workflows/release.yml:227`).
  They have no speech, no CUDA and no NVENC.
- CUDA, Vulkan and NVENC are opt-in Cargo features (`crates/horizon-ui/Cargo.toml:11-22`).
  The build does not find a GPU toolkit automatically.
- A Cargo `build.rs` file cannot turn on a feature. A wrapper or a runtime
  backend load is necessary for automatic GPU builds.
- No script installs git, Git LFS, rustup, NASM or the system headers for a user.
  `scripts/install-ci-ubuntu-dependencies.sh` refuses to run outside CI.
- The Quick Start in `AGENTS.md` does not list macOS arm64 and does not tell the
  reader to run `git lfs pull`. `README.md` lists both.
- The in-app updater works for Surge installs only.

### Agent support

- The plugin has three skills: `horizon-browser`, `horizon-device` and
  `horizon-speech`. There is no setup, update or doctor skill.
- Horizon writes its skills to disk when it starts. Thus an agent cannot use a
  Horizon skill before Horizon is installed.
- The only doctor command is `horizon-device doctor`. It examines VNC targets only.

### First start

- There is no welcome board, tour or first-run check.
- `README.md` has 865 lines. It does not mention clouds, casting or the
  Local Network Bridge.

### Clouds

A first cloud can need these credentials:

| Credential | Need | Where the user puts it |
|---|---|---|
| RunPod API key or Hetzner API token | Required | **Cloud settings** |
| Registry push and pull logins | Required in practice | **Cloud settings**, plus local `docker login` |
| Codex or Claude API key, or subscription login | One for each agent | **Cloud settings** |
| Tailscale auth key | Optional | **Settings > Tailnets** |
| GitHub token for push from the worker | Optional | Manual edit of `settings.json` |
| Clone token for a private repository | Optional | **New cloud** token card |

- There is no public default worker image. Each example profile uses
  `registry.example.com`. This is the largest obstacle for a first cloud.
- Horizon does not examine a provider key with a live API call before start.
- Tailscale accepts auth keys only. There is no OAuth client.

### GitHub

- **New cloud** accepts a pasted link or a local folder. There is no GitHub
  sign-in and no repository picker.
- A fresh clone shows phase, percent, speed and time left. The user can cancel
  and continue it.
- Horizon does not fetch an existing local clone from `origin` before deploy.
  Deploy sends only committed source. Uncommitted changes stay on the PC.
- **Committed base revision** is a free-text field with the default `HEAD`.
- There is no general cancel for a source upload.

### Device control from Linux

- A Device panel can show a Mac desktop through `ssh -W` to a loopback VNC port.
- Horizon has no Mac companion and no iOS Simulator tool. The simulator
  scripts in `horizon-app` are private and are not a Horizon function.

## Why clouds are important

A cloud gives each agent its own disposable machine. The value is as follows:

1. The agents continue when the laptop sleeps or goes offline.
2. The user can rent a GPU for one hour. The user does not buy one.
3. The agent cannot damage the PC, because it works on the worker.
4. Many agents can work on many repositories at the same time.
5. Terminal, browser and Device panels on the worker look the same as local panels.
6. The Local Network Bridge lets a cloud reach devices on the local network.
7. Idle stop and delete keep the cost low.

## Goals

| Measure | Now | Target |
|---|---|---|
| Steps from zero to a running board | About 6, with a toolkit | 1 command or 1 agent request |
| Credentials for a first cloud | 3 to 6 | 1 provider key and 1 agent sign-in |
| GPU speed in a release build | CPU only, no speech | GPU when the machine has one |
| Time to the first cloud | Not measured | 10 minutes or less |
| Functions that a new user can find in the app | Board, terminal | All functions in this plan |

## Work items

### Phase 1: Install in one step

- [ ] **1.1 Doctor command.** Add `horizon doctor` and a `horizon_doctor` MCP
      tool. Report GPU, driver, CUDA SM, Vulkan, FFmpeg encoders, microphone,
      browsers, git, `gh`, Tailscale and credential state. Give JSON output for agents.
- [ ] **1.2 GPU in release builds.** Put speech in every release. Load the CUDA
      and Vulkan backends at runtime, with CPU fallback. If runtime load is not
      possible, publish a CUDA variant and let the installer select it.
- [ ] **1.3 GPU detection for source builds.** Add `cargo xtask build`. It finds
      the CUDA toolkit, the Vulkan SDK and the GPU SM, then selects the features.
      Make `build.rs` show a warning when it finds a toolkit that the build does not use.
- [ ] **1.4 Bootstrap scripts.** Add `install.sh` and `install.ps1`. They install
      a release binary by default. With `--from-source`, they install git, Git
      LFS, rustup, NASM, CMake and the system headers. Show the plan first.
- [ ] **1.5 Setup and update skills.** Add `horizon-setup` and `horizon-update`
      skills. Publish them in a plugin marketplace in this repository. Then an
      agent can install Horizon before Horizon runs.
- [ ] **1.6 Correct the Quick Start.** Add macOS arm64 and `git lfs pull` to
      `AGENTS.md`. Use one install source for `README.md` and `AGENTS.md`.

### Phase 2: The first ten minutes

- [ ] **2.1 Welcome board.** On the first start, open a sample workspace. It has
      a terminal, an agent, a browser and a Device panel, each with a **Try it** action.
- [ ] **2.2 Function cards.** Add a **What can Horizon do?** view. Show one short
      video for each function: browser control, VNC, iOS Simulator, cloud,
      casting and dictation.
- [ ] **2.3 New README.** Start with the functions and a 60-second video. Move
      the reference text to `docs/`. Write it in STE.
- [ ] **2.4 First steps guide.** Add `docs/first-steps.md` in STE, with a test
      procedure under `docs/testing/procedures/`.

### Phase 3: The first cloud in ten minutes

- [ ] **3.1 Public worker image.** Publish a signed default worker image. Then a
      first cloud needs no registry and no local Docker.
- [ ] **3.2 Cloud setup wizard.** Ask for one credential in each step. Give a
      **Create key** link and examine each key with a live API call.
- [ ] **3.3 Credential import.** Find existing credentials, for example `gh auth`,
      `RUNPOD_API_KEY` and `HCLOUD_TOKEN`. Import a credential only after the user agrees.
- [ ] **3.4 Tailscale OAuth.** Accept a Tailscale OAuth client as an alternative to
      an auth key. Make auth keys for each worker automatically.
- [ ] **3.5 Git push credential in the UI.** Replace the manual `settings.json`
      edit with a field in **Cloud settings**.
- [ ] **3.6 Cost preview.** Show the cost for one hour and for one month before
      the first start. Turn on idle stop for a first cloud by default.

### Phase 4: GitHub integration

- [ ] **4.1 GitHub sign-in.** Use the GitHub device flow, or reuse a `gh` login.
- [ ] **4.2 Repository picker.** Search the user's repositories and
      organizations. Replace the free-text revision with a branch and commit picker.
- [ ] **4.3 Update from origin.** Add the checkbox **Update from origin before
      deploy**. Fetch, show ahead and behind counts, and fast-forward only a clean branch.
- [ ] **4.4 Uncommitted changes.** Before deploy, show the uncommitted files.
      Tell the user that deploy does not send them.
- [ ] **4.5 Cancel at all stages.** Add **Cancel** to source upload, image pull
      and deploy. Show progress with bytes, speed and time left in each stage.

### Phase 5: Control a Mac from Linux

- [ ] **5.1 Mac companion.** Add one setup command for a Mac. It turns on
      loopback VNC and SSH, and it adds the Mac to **Remote Hosts**.
- [ ] **5.2 iOS Simulator tools.** Add MCP tools to boot a simulator, install an
      app, take a screenshot and record a video. Show the simulator in a Device panel.
- [ ] **5.3 Procedure.** Write an STE test procedure for the Linux to Mac lane.

### Phase 6: Documentation in STE

- [x] **6.1 STE default.** Make STE the default for all documentation in
      `AGENTS.md` and `docs/style/ste-rules.md`.
- [ ] **6.2 Glossary.** Add the new names from this epic to
      `docs/style/technical-names.md`.
- [ ] **6.3 Prose lint.** Add Vale with an STE style to CI. Tracked in #1265.
- [ ] **6.4 UI test procedure for each item.** Each item in Phases 1 to 5 is
      complete only with an STE test procedure in `docs/testing/procedures/`.
      Run it on an isolated desktop in a Device panel. Attach the PR GIF.
- [ ] **6.5 First-run procedure on a clean machine.** Write one STE procedure
      for the full path: install, doctor, welcome board, first cloud. Run it on
      a clean Linux, macOS and Windows machine for each release.
- [ ] **6.6 Procedures that an agent can run.** Give each step a fixed form: an
      action, a UI label in bold and a `Result:` line. An agent can then do the
      steps with `horizon-device` or the `browser_*` tools and write the report.
- [ ] **6.7 Screenshot for each result.** Give each `Result:` line a reference
      screenshot. A run compares its screenshot with the reference and records
      the difference in the report.
- [ ] **6.8 STE for UI text.** Write labels, error messages, wizard steps and
      empty states in STE. Use the names in `technical-names.md`. The welcome
      board and the cloud wizard (2.1, 3.2) are the first users.

## Order

Do Phase 1 first. The doctor command (1.1) gives the data for the installer,
the skills, the welcome board and the cloud wizard. Phases 2, 3 and 4 can then
start at the same time. Phase 5 needs a Mac for each test run.

## Risks

> **CAUTION:** DO NOT PUT A CREDENTIAL IN A LOG, AN ISSUE OR THE REPOSITORY. The
> import (3.3) and the wizard (3.2) touch secrets.

- A CUDA binary that links `libcudart` does not start on a machine without CUDA.
  Use runtime load or a separate variant. Do not make it the only binary.
- A fast-forward before deploy can change the local branch. Do it only on a
  clean branch, and only when the user selects the checkbox.
- A public worker image must have no secrets and must have a signature.
