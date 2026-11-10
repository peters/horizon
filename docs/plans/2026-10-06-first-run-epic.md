# Epic: Easy first run

Status: proposed, 2026-10-06. Tracked in #1311. This plan uses ASD-STE100
Simplified Technical English. See [the STE rules](../style/ste-rules.md).

## Purpose

A new user must get from zero to a useful Horizon board in a short time. The
user must not need to be a programmer. An agent must be able to do the full
setup for the user on Linux, macOS and Windows.

This epic also shows new users what Horizon can do. Most users do not know that
an agent can control a browser or a VNC desktop from one board.
Control of an iOS Simulator from Linux is planned in Phase 5. It is not
available today.

## Current state (audit of `origin/main` at `7c31b9c91`)

### Install and build

- Release binaries use the default features only (`.github/workflows/release.yml:227`).
- No script installs git, Git LFS, rustup, NASM or the system headers for a user.
  `scripts/install-ci-ubuntu-dependencies.sh` refuses to run outside CI.
- The Quick Start in `AGENTS.md` did not list macOS arm64 and did not tell the
  reader to run `git lfs pull`. Phase 0 corrects this.
- Horizon has no in-app updater. Users replace the release binary or use their package manager.
- The latest release, v0.2.7 from 2 August 2026, is more than 800 commits older
  than `main`. It has no browser panels or Device panels. A user who installs a
  release does not get the functions that the README shows.

### Agent support

- The plugin has two skills: `horizon-browser` and `horizon-device`. There is no
  setup, update or doctor skill.
- Horizon writes its skills to disk when it starts. Thus an agent cannot use a
  Horizon skill before Horizon is installed.
- The only doctor command is `horizon-device doctor`. It examines VNC targets only.

### First start

- There is no welcome board, tour or first-run check.
- `README.md` has 865 lines. It did not mention agent-driven desktops. Phase 0
  adds short links to them.

### Device control from Linux

- A Device panel can show a Mac desktop through `ssh -W` to a loopback VNC port.
- Horizon has no iOS Simulator tool.

### Platform support

Many functions work on Linux only, and the README did not say so. On Windows,
agent panels need a POSIX shell in `SHELL`. [Platform support](../platform-support.md)
gives the full list with the code references.

This epic does not plan new platform support. It documents the limits, and it
makes the first run clear on each platform.

## Goals

| Measure | Now | Target |
|---|---|---|
| Steps from zero to a running board | About 6, with a toolkit | 1 command or 1 agent request |
| Functions that a new user can find in the app | Board, terminal | All functions in this plan |

## Work items

### Phase 0: Document what exists

- [x] **0.1 Platform support.** Add `docs/platform-support.md` with the limits of
      each function on Linux, macOS and Windows, and the code references.
- [x] **0.2 First steps guide.** Add `docs/first-steps.md` in STE. It tells
      the user how to install Horizon and use the board.
- [x] **0.3 README.** Add a platform table, a link to the first steps guide, and
      the functions that the README did not give: agent-driven desktops. Tell
      that the Snap publication is paused.
- [x] **0.4 Correct the Quick Start.** Add macOS arm64, Git LFS and the platform
      limits to `AGENTS.md`.
- [x] **0.5 Skill text.** Tell in the `horizon-device` skill that an isolated
      desktop works on Linux with X11 only.

### Phase 1: Install in one step

- [ ] **1.1 Doctor command.** Add `horizon doctor` and a `horizon_doctor` MCP
      tool. Report the GPU, the driver, the FFmpeg encoders, the microphone,
      the browsers, git, `gh` and credential state. Give JSON output for agents.
- [ ] **1.2 Bootstrap scripts.** Add `install.sh` and `install.ps1`. They install
      a release binary by default. With `--from-source`, they install git, Git
      LFS, rustup, NASM and the system headers. Show the plan first.
- [ ] **1.3 Setup and update skills.** Add `horizon-setup` and `horizon-update`
      skills. Publish them in a plugin marketplace in this repository. Then an
      agent can install Horizon before Horizon runs.
- [ ] **1.4 Release the current code.** Publish a new release, or a prerelease
      channel, from `main`. Then the release routes give the functions in the
      documentation.

### Phase 2: The first ten minutes

- [ ] **2.1 Welcome board.** On the first start, open a sample workspace. It has
      a terminal, an agent, a browser and a Device panel, each with a **Try it** action.
      Show only the functions that work on the platform of the user.
- [ ] **2.2 Function cards.** Add a **What can Horizon do?** view. Show one short
      video for each function: browser control, VNC and iOS Simulator.
- [ ] **2.3 Shorter README.** Start with the functions and a 60-second video.
      Move the reference text to STE documents in `docs/`. Write the README in
      plain, friendly language. The README is not in STE.

### Phase 4: Control a Mac from Linux

- [ ] **4.1 Remote host setup.** Add one setup command for a Mac. It turns on
      loopback VNC and SSH, and it adds the Mac to **Remote Hosts**.
- [ ] **4.2 iOS Simulator tools.** Add MCP tools to boot a simulator, install an
      app, take a screenshot and record a video. Show the simulator in a Device panel.
- [ ] **4.3 Procedure.** Write an STE test procedure for the Linux to Mac lane.

### Phase 5: Documentation in STE

- [x] **5.1 STE default.** Make STE the default for the technical
      documentation in `AGENTS.md` and `docs/style/ste-rules.md`. The README is
      not in the scope.
- [x] **5.2 Glossary.** Add the install and build names to
      `docs/style/technical-names.md`.
- [ ] **5.3 Prose lint.** Add Vale with an STE style to CI. Tracked in #1265.
- [ ] **5.4 UI test procedure for each item.** Each item in Phases 1 to 4 is
      complete only with an STE test procedure in `docs/testing/procedures/`.
      Run it on an isolated desktop in a Device panel. Attach the PR GIF.
- [ ] **5.5 First-run procedure on a clean machine.** Write one STE procedure
      for the full path: install, doctor and welcome board. Run it on a clean
      Linux, macOS and Windows machine for each release.
- [ ] **5.6 Procedures that an agent can run.** Give each step a fixed form: an
      action, a UI label in bold and a `Result:` line. An agent can then do the
      steps with `horizon-device` or the `browser_*` tools and write the report.
- [ ] **5.7 Screenshot for each result.** Give each `Result:` line a reference
      screenshot. A run compares its screenshot with the reference and records
      the difference in the report.
- [ ] **5.8 The same names in the UI.** Use the names in `technical-names.md`
      for labels, error messages, wizard steps and empty states. Write them in
      plain language. The welcome board (2.1) is the first user.

## Order

Phase 0 is complete in the first PR. Do Phase 1 next. The doctor command (1.1)
gives the data for the installer, the skills and the welcome board. Phase 2 can
then start. Phase 4 needs a Mac for each test run. Phase 5 applies to all
phases.

## Risks

- A binary that links a GPU library does not start on a machine without that
  library. Use runtime load or a separate variant. Do not make it the only
  binary.
