# Platform support

This document tells which Horizon functions work on Linux, macOS and Windows.
It describes the code on `main` at commit `7c31b9c91`. It does not give a plan.
Each limit has a reference to the code or to the workflow that causes it.

Horizon builds and runs on the three platforms. Many functions work on Linux
only. If you change the platform support of a function, update this document in
the same PR.

## Values in the tables

| Value | Meaning |
|---|---|
| Yes | The function works. |
| Partial | Part of the function works. The note tells which part. |
| No | The function is not available. Horizon shows an error or hides it. |
| Not tested | No code blocks the function, but no test or procedure examines it. |

## Panels and the board

| Function | Linux | macOS | Windows |
|---|---|---|---|
| Board, workspaces, sessions | Yes | Yes | Yes |
| Shell panels | Yes | Yes | Partial (note 1) |
| Agent panels | Yes | Yes | Not tested (note 2) |
| Browser panels: Chromium and Firefox | Yes | Yes | Yes |
| Browser panels: Safari | No | Yes | No |
| BrowserStack browsers and phones | Yes | Yes | Yes |
| Device panels (VNC viewer) | Yes | Yes | Yes |
| Agent input to an isolated desktop | Partial (note 3) | No | No |
| Remote Hosts: SSH and Tailscale | Yes | Yes | Not tested |
| Native image paste into a terminal | Yes | Partial (note 4) | Partial (note 4) |
| Panel screenshot and image copy | Yes | Yes | Yes |

1. On Windows, a shell panel starts the program in the `SHELL` variable. If
   `SHELL` is not set, it starts `/bin/bash`
   (`crates/horizon-core/src/panel/spawn.rs:624-634`). A usual Windows
   installation does not have this program. Windows also gives no child process
   ID, so Horizon does not follow the working directory of the shell
   (`crates/horizon-core/src/terminal/lifecycle.rs:60-63`).
2. Horizon starts each agent through `$SHELL -ic`
   (`crates/horizon-core/src/panel/spawn.rs:517,584-593`). There is no
   Windows launcher. If `SHELL` is not set, Horizon uses `/bin/bash` (note 1).
   A POSIX shell in `SHELL`, for example Git Bash, can work, but no test
   examines it. Issue #688 has the history.
3. The `horizon-device` CLI and MCP tools send input to a local X11 display only.
   They do not work on Wayland without X11. On other platforms they return
   `only local X11 is implemented` (`crates/horizon-device/src/lib.rs:83-98`).
   The Device panel viewer works on all platforms.
4. On macOS and Windows, a file drop uses the usual window file drop. The native
   positioned image paste is a Linux function (`crates/horizon-ui/src/app/file_drop.rs`).

## Clouds

| Function | Linux | macOS | Windows |
|---|---|---|---|
| Clouds: deploy, stop, resume, delete | Yes | Yes | No (note 5) |
| Tailnets | Yes | Yes | No (note 5) |
| Local Network Bridge | Yes | Yes | No (note 5) |
| Credential store for tailnet auth keys | Secret Service | Keychain | Credential Manager |

5. Horizon refuses all cloud operations on a host that is not Unix
   (`crates/horizon-core/src/session_store.rs:550-559`). Tailnets and the Local
   Network Bridge need a cloud. Issue #969 tracks Windows support.

## Media and speech

| Function | Linux | macOS | Windows |
|---|---|---|---|
| Apple TV casting | Yes | No | No |
| NVENC hardware encoder for casting | Opt-in (note 6) | No | No |
| Chromecast | No (note 7) | No (note 7) | No (note 7) |
| Speech in a release build | No (note 8) | No (note 8) | No (note 8) |
| Speech in a source build, CPU | Yes | Yes | Not tested |
| Speech in a source build, GPU | CUDA or Vulkan | Metal | CUDA or Vulkan, not tested |
| Push-to-talk in Horizon windows | Yes | Yes | Not tested |
| Global push-to-talk and text in other applications | X11 only | Yes (note 9) | No |

6. Build with the `cast-nvenc` feature. The encoder needs the NVIDIA driver and
   an FFmpeg with `h264_nvenc`. See [casting](casting.md).
7. The `horizon-chromecast` crate is a library. The Horizon application does not
   use it yet.
8. The release workflow builds with the default features only
   (`.github/workflows/release.yml:227`). Speech is an opt-in feature. To get
   speech, build from source with `cargo speech`. See the README.
9. macOS asks for the Accessibility permission.

## Install and update

| Function | Linux | macOS | Windows |
|---|---|---|---|
| Release binary | x64 | arm64 and x64 | x64 |
| Surge installer and in-app update | x64 | arm64 and x64 | x64 |
| Homebrew | x64 | Yes | No |
| WinGet | No | No | Yes |
| Snap Store | No (note 10) | No | No |
| Code signing and notarization in the release | n/a | No (note 11) | No (note 11) |

10. The Snap publish job has `if: false` (`.github/workflows/release.yml:569-573`).
11. The release workflow and the packaging scripts have no signing or
    notarization step.

## Agent skills

Horizon installs the `horizon-browser`, `horizon-device` and `horizon-speech`
skills on all platforms. The `horizon-device` skill tells agents to drive an
isolated desktop. This part works on Linux with X11 only (note 3).

## Unknown

These items have no test and no clear code limit:

- Remote Hosts, Taildrop and `ssh -W` VNC tunnels with Windows OpenSSH.
- Speech with Vulkan on macOS.
- Shell and agent panels on Windows when `SHELL` points to Git Bash or MSYS.
