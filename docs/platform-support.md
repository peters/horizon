# Platform support

Horizon supports Linux and macOS. Windows is not a supported platform: the
workflows build and test Linux and macOS only, and a release publishes no
Windows asset. The Windows build, test and release lanes are gone, and the
platform gate that chose a Windows code path is gone with them.

Two kinds of Windows value stay. A BrowserStack target device is a remote
device, not the platform Horizon runs on. Horizon also reads `USERPROFILE` when
`HOME` is unset. A normal Linux or macOS start sets `HOME`, so this fallback
does not change it. The Docker restart plan and the OpenCode lookup hold no
Windows value.

This document tells which Horizon functions work on Linux and macOS. It
describes the code on `main`. It does not give a plan. Each note gives the code,
workflow or document that causes a limit. The "Unknown" section lists the items
that no test examines.

The latest release, v0.2.7 from 2 August 2026, is older than this code. It
does not have browser panels or Device panels. The tables describe a source
build from `main`.

Many functions work on Linux only. If you change the platform support of a
function, update this document in the same PR.

## Values in the tables

| Value | Meaning |
|---|---|
| Yes | The function works. |
| Partial | Part of the function works. The note tells which part. |
| No | The function is not available. Horizon shows an error or hides it. |
| Not tested | No code blocks the function, but no test or procedure examines it. |

## Panels and the board

| Function | Linux | macOS |
|---|---|---|
| Board, workspaces, sessions | Yes | Yes |
| Shell panels | Yes | Yes |
| Agent panels | Yes | Yes |
| Browser panels: Chromium and Firefox | Yes | Yes |
| Browser panels: Safari | No (note 1) | Yes |
| File drops into local Chromium and Firefox panels | Yes | Not tested |
| Passkey bridge in local Chromium and Firefox panels | Yes | Not tested |
| BrowserStack browsers and phones | Yes | Yes |
| Device panels (VNC viewer) | Yes | Yes |
| Agent input to an isolated desktop | Partial (note 2) | No (note 2) |
| Remote Hosts: SSH and Tailscale | Yes | Yes |
| Native image paste into a terminal | Yes | Partial (note 3) |
| Panel screenshot and image copy | Yes | Yes |

1. Safari WebDriver is available only on macOS
   (`crates/horizon-browser/src/webdriver/service.rs:60-62`).
2. The `horizon-device` CLI and MCP tools send input to a local X11 display only.
   They do not work on Wayland without X11. On other platforms they return
   `only local X11 is implemented` (`crates/horizon-device/src/lib.rs:97`).
   The Device panel viewer works on both platforms.
3. A file drop uses the usual window file drop. The native positioned image
   paste is a Linux function (`crates/horizon-ui/src/app/file_drop.rs`).

A shell panel starts the program in the `SHELL` variable. Without `SHELL` it
starts `/bin/zsh` on macOS and `/bin/bash` on Linux
(`crates/horizon-core/src/panel/spawn.rs:647-651`). Horizon starts each agent
through `$SHELL -ic` (`crates/horizon-core/src/panel/spawn.rs:616`).

## Install and update

| Function | Linux | macOS |
|---|---|---|
| Release binary (note 1) | x64 | arm64 and x64 |
| Homebrew | x64 | Yes |
| Code signing and notarization in the release | n/a | No (note 2) |

1. The release assets and the Homebrew tap come from the release workflow. See
   [the release flow](release-flow.md).
2. The release workflow and the packaging scripts have no signing or
   notarization step.

## Agent skills

Horizon installs the `horizon-browser` and `horizon-device` skills on both
platforms. The `horizon-device` skill tells agents to drive an
isolated desktop. This part works on Linux with X11 only (note 2 under
"Panels and the board").
