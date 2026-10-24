# Platform support

Horizon supports Linux and macOS. Windows is not a supported platform: the code
has no Windows paths, the workflows build and test Linux and macOS only, and a
release publishes no Windows asset.

This document tells which Horizon functions work on Linux and macOS. It
describes the code on `main`. It does not give a plan. Each note gives the code,
workflow or document that causes a limit. The "Unknown" section lists the items
that no test examines.

The latest release, v0.2.7 from 2 August 2026, is older than this code. It
does not have browser panels, Device panels, clouds, tailnets or casting. The
tables describe a source build from `main`.

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

## Clouds

| Function | Linux | macOS |
|---|---|---|
| Clouds: deploy, stop, resume, delete | Yes | Yes |
| Tailnets | Yes | Yes |
| Local Network Bridge | Yes | Yes |
| Credential store for tailnet auth keys | Secret Service | Keychain |

## Media and speech

| Function | Linux | macOS |
|---|---|---|
| Apple TV casting | Yes | No (note 1) |
| NVENC hardware encoder for casting | Opt-in (note 2) | No |
| Chromecast | No (note 3) | No (note 3) |
| Speech in a release build | No (note 4) | No (note 4) |
| Speech in a source build, CPU | Yes | Yes |
| Speech in a source build, GPU | CUDA or Vulkan | Metal. Vulkan not tested |
| Push-to-talk in Horizon windows | Yes | Yes |
| Global push-to-talk and text in other applications | X11 only (note 5) | Yes (note 5) |

1. The casting module and the `horizon-cast` crate are Linux-only
   (`crates/horizon-ui/src/app/mod.rs:20-21`, `crates/horizon-ui/Cargo.toml`).
   See [casting](casting.md).
2. Build with the `cast-nvenc` feature. The encoder needs the NVIDIA driver and
   an FFmpeg with `h264_nvenc`. See [casting](casting.md).
3. The `horizon-chromecast` crate is a library. The Horizon application does not
   use it yet.
4. The release workflow builds with the default features only
   (`.github/workflows/release.yml:176`). Speech is an opt-in feature. To get
   speech, build from source with `cargo speech`. See the README.
5. The global hotkey exists for X11 Linux and macOS only
   (`crates/horizon-cursor/src/hotkey.rs:136,547`). macOS asks for the
   Accessibility permission.

## Install and update

| Function | Linux | macOS |
|---|---|---|
| Release binary (note 1) | x64 | arm64 and x64 |
| Homebrew | x64 | Yes |
| Snap Store | Partial (note 2) | No |
| Code signing and notarization in the release | n/a | No (note 3) |

1. The release assets and the Homebrew tap come from the release workflow. See
   [the release flow](release-flow.md).
2. The Snap publish job has `if: false` in `.github/workflows/release.yml`.
   The store can still hold an older release. New releases do not go to the store.
3. The release workflow and the packaging scripts have no signing or
   notarization step.

## Agent skills

Horizon installs the `horizon-browser`, `horizon-device` and `horizon-speech`
skills on both platforms. The `horizon-device` skill tells agents to drive an
isolated desktop. This part works on Linux with X11 only (note 2 under
"Panels and the board").

## Unknown

These items have no test and no clear code limit:

- Speech with Vulkan on macOS.
