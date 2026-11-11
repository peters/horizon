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
| Browser panel of a browser on another machine | Yes (note 4) | Yes (note 4) |
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
4. The client needs the OpenSSH client. The host machine needs an `sshd` that
   answers without a prompt, Chrome or Chromium, and a graphical login session,
   because Horizon starts that browser headed with no window of its own
   (`crates/horizon-browser-control/src/ssh_forward/remote_browser.rs`). The
   path is Chromium only: a Firefox or Safari create returns
   `unsupported_backend` (`crates/horizon-core/src/browser/attached_host.rs`).
   The live lane of this path used a macOS host. No run examined a Linux host,
   so that lane is "Not tested". Test it with
   [the remote browser tunnel procedure](testing/procedures/remote-browser-tunnel.md).

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

## Secret values

Every secret value of Horizon is a row of `<Horizon home>/secrets.sqlite`, on both
platforms. Horizon therefore needs no keyring service, no Secret Service over
D-Bus and no Keychain, and a Linux install needs no desktop session for a
credential. The file mode is the protection, as
[the boundary of the named secrets](review/trust-boundaries/named-secrets.md)
states.

### The keyring import

An older version kept the value of a remote browser provider in the system
keyring. The tab **Remote** reads such a value one time, when a person presses
**Import from the keyring**. No crate of this workspace opens the keyring: three
programs of the system answer instead.

| System | Program | Function | Install |
|---|---|---|---|
| Linux | `secret-tool` | Reads every entry of the service `horizon-remote-browser`. | The package `libsecret-tools`. |
| Linux, without `secret-tool` | `python3` with the `gi` binding of libsecret | Reads the same entries and prints one JSON document. | The package of the binding, such as `python3-gi` with `gir1.2-secret-1`. |
| macOS | `security` | Reads one entry for each account of the settings. | Part of macOS. |

Both Linux programs read the Secret Service over the session bus, so an import
needs a session bus, such as the one of a desktop session. A headless install
without a keyring service therefore imports nothing: the tab says that this
computer has no keyring service or that the program is missing, and the person
types the value in the tab.

On macOS the system can ask the person for permission for each value, because the
Keychain belongs to that person. The import waits up to 60 seconds for one entry,
and up to 20 seconds for the whole import on Linux.

When the program of a system is not found, the line under the button says what
that system needs. Linux names the package `libsecret-tools`. macOS names the
program `security`, which is a part of macOS.

## Speech input

Horizon holds no speech engine, no audio library and no model. It runs a
recorder program that the person names, and it sends the recording to a
transcription service over HTTPS or to a transcription command
(`crates/horizon-core/src/speech/`). The program and the device decide what
works, so the platform gives the default and the recipe.

| Function | Linux | macOS |
|---|---|---|
| The hotkey, the state mark and the insert of the text | Yes | Yes |
| The default recorder line | Yes: `pw-record` of PipeWire records 16 kHz on one channel | No: the line is empty, and the person installs a recorder |
| A recorder recipe that the documents give | `pw-record --rate 16000 --channels 1 --format s16 {file}` | `ffmpeg -f avfoundation -i :default -ac 1 -ar 16000 -y {file}`, or `rec -c 1 -r 16000 {file}` |
| A take with a real microphone | Yes with PipeWire | Not tested: no run of this repository used a real microphone on macOS |
| The transcription service over HTTPS | Yes | Yes |
| The transcription command | Yes | Yes |

A Linux computer needs a PipeWire session, because `pw-record` records from it.
The stand-in recorder of the fixture,
`scripts/device-smoke/speech_standin_recorder.py`, copies one prepared file and
opens no audio device, so a test needs no microphone on either platform. See
[the speech input procedure](testing/procedures/speech-input.md).

## Agent skills

Horizon installs the `horizon-browser`, `horizon-device` and `horizon-speech`
skills on both platforms. The `horizon-speech` skill says what a take needs on
each platform, the recorder recipes, the named secret of the key, and what leaves
the computer. The `horizon-device` skill tells agents to drive an
isolated desktop. This part works on Linux with X11 only (note 2 under
"Panels and the board").
