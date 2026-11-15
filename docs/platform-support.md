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
| Agent state from the program (note 10) | Yes | Yes |
| Continue work after restart (note 6) | Yes | Yes |
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
| Two-finger pinch to zoom the canvas | Partial (note 7) | No (note 7) |
| Middle-click paste of the primary selection | Yes (note 8) | No (note 8) |
| Panel screenshot and image copy | Yes | Yes |
| Desktop notification of an agent | Partial (note 5) | Yes (note 5) |
| Desktop entry and app icon | Yes (note 9) | Partial (note 9) |

1. Safari WebDriver is available only on macOS
   (`crates/horizon-browser/src/webdriver/service.rs:270-286`).
2. The `horizon-device` CLI and MCP tools send input to a local X11 display only.
   They do not work on Wayland without X11. On other platforms they return
   `only local X11 is implemented` (`crates/horizon-device/src/lib.rs:95-98`).
   The Device panel viewer works on both platforms.
3. A file drop uses the usual window file drop. The native positioned image
   paste is a Linux function (`crates/horizon-ui/src/native_window.rs:293-320`,
   `crates/horizon-ui/src/input/native_files.rs`). On macOS Cmd+V pastes text
   and no image.
4. The client needs the OpenSSH client. The host machine needs an `sshd` that
   answers without a prompt, Chrome or Chromium, and a graphical login session,
   because Horizon starts that browser headed with no window of its own
   (`crates/horizon-browser-control/src/ssh_forward/remote_browser.rs`). The
   path is Chromium only: a Firefox or Safari create returns
   `unsupported_backend` (`crates/horizon-core/src/browser/attached_host.rs`).
   The live lane of this path used a macOS host. No run examined a Linux host,
   so that lane is "Not tested". Test it with
   [the remote browser tunnel procedure](testing/procedures/remote-browser-tunnel.md).
5. Horizon shows a notification with `notify-send` on Linux and with `osascript`
   on macOS (`crates/horizon-core/src/notify/sink.rs:67-93`). A Linux computer
   without `notify-send` sends nothing, and Settings → General says so under the
   row. The two programs are the only dependencies. No test examines the line of
   macOS, so that lane is "Not tested". Test both systems with
   [the desktop notification procedure](testing/procedures/desktop-notifications.md).
6. Horizon continues the work of a Claude panel after a restart when the panel
   allows it and the evidence proves a clean suspension
   (`crates/horizon-core/src/agent_work/lifecycle.rs:67-76`). The rule is the
   same on both systems. The handoff and continuation tests pass on Linux and on
   a real Mac, and the process checks that they need answer on both: signal 0
   names a process that exists, and `/proc` or `ps` names its start time. A Mac
   cannot read the command line of a live process without a program, so an entry
   that names a live process asks the person for a decision. No run has watched
   the live lane of that path on a Mac yet, so that lane is "Not tested" there.
7. The two-finger pinch turns the canvas only on Linux Wayland, through the
   `libwayland` bridge of this workspace (`crates/horizon-wayland/src/lib.rs`).
   Linux X11 has no pinch, and macOS has none: its toolkit sends no magnify
   event (`crates/horizon-ui/src/native_window.rs:150-160`). Ctrl/Cmd+scroll
   zooms, and **Ctrl+0** resets the zoom on both.
8. A middle click pastes the primary selection on Linux, from the X11 or the
   Wayland selection (`crates/horizon-ui/src/terminal_widget/input.rs:745-751`,
   `crates/horizon-ui/src/primary_selection.rs`). macOS has no primary
   selection, so a middle click does nothing.
9. On Linux a tarball or a `cargo run` window installs
   `~/.local/share/applications/horizon.desktop` and the icons, so the launcher
   shows the name and the icon
   (`crates/horizon-ui/src/agent_integrations.rs:36-41`). macOS uses a bundle:
   `packaging/macos/make_app_bundle.sh` writes `Horizon.app` with the icon and
   the `Info.plist`, and `--install` copies it to the applications folder. The
   release asset is a bare binary, so a downloaded copy shows a generic icon in
   the Dock until the person makes the bundle (note 3 under "Install and
   update").

10. Claude Code, pi and Codex report the state of their turns to the sidebar on
    both systems. Horizon gives a Claude launch the plugin folder of the work
    hooks with `--plugin-dir`, and a pi launch one extension file of its own home
    with `-e` (`crates/horizon-core/src/agent_work/lifecycle.rs`,
    `crates/horizon-ui/src/plugin_install/work_extension.rs`). The pi file is
    written at the start of Horizon, and a launch adds it to the command line
    only while it is a file, because pi does not start at a path that names no
    file. The file must be on the computer that runs pi: Horizon writes it into
    the home of its own computer, so a pi panel of another computer would need its
    own copy there, and Horizon does not place one today. A Codex launch needs no
    argument, no file and no setting of Codex: Codex reports its state with the
    program status protocol (OSC 7501), which the reader of the panel's terminal
    hands over (`crates/horizon-pty-tap`,
    `crates/horizon-core/src/agent_work/program_status.rs`), and it also writes
    the state into the title of that terminal
    (`crates/horizon-core/src/agent_work/title.rs`), which the panel reads when no
    report arrives. The reader of the panel is the one place of the product that
    carries a scoped `unsafe` function, because the interface of the terminal
    library declares the method it forwards as `unsafe`. A `tmux` between Codex
    and Horizon drops the report of the protocol, and it keeps the titles of its
    panes, so such a panel reports only when that `tmux` sets `set-titles on` and
    `set-titles-string '#{pane_title}'`; the default setting of `tmux` gives no
    title, and its default string prefixes the pane title with the session name,
    which Horizon reads as no state. The other agents keep the rule of their
    screen on both systems.

A shell panel starts the program in the `SHELL` variable. Without `SHELL` it
starts `/bin/zsh` on macOS and `/bin/bash` on Linux
(`crates/horizon-core/src/panel/spawn.rs:764-777`). Horizon starts each agent
through `$SHELL -ic` (`crates/horizon-core/src/panel/spawn.rs:738-742`).

## Machines and their agents

Horizon watches the agents that run in `tmux` on the machines of its machine
list, with no panel open (`crates/horizon-core/src/machine_watch.rs`,
[docs/architecture/machine-watch.md](architecture/machine-watch.md)). This
computer starts one `ssh` child per machine, and that child runs one text of
POSIX shell on the machine.

| Function | Linux | macOS |
|---|---|---|
| Watch the agents of a machine | Yes | Yes |
| A machine with no `tmux` or no `tmux` server | Yes (note 1) | Yes (note 1) |
| An agent that another program starts through an interpreter | Partial (note 2) | Partial (note 2) |
| The text of the watch under `dash`, `bash` and the shell of `BusyBox` | Yes | Partial (note 3) |

1. A machine without `tmux` or without a `tmux` server is a machine whose watch
   reports that in one line. The watch keeps that machine in the list.
2. The program of a pane answers first. When an interpreter such as `node` or
   `python3` runs there, the watch reads the command lines of the processes of
   that pane's terminal with `ps -t`, which GNU `ps` and the `ps` of macOS have.
   A small appliance with a `BusyBox` `ps` has no `-t`, so such a machine names
   the interpreter alone and an agent behind one keeps no row. The text of the
   watch still runs there.
3. The `tmux` of the machine decides whether the title of a pane keeps a
   character outside ASCII. The text of the watch asks `tmux` with `-u`, so a
   shell with no UTF-8 locale, which an `ssh` command often is, still reads the
   braille frame of a Codex title, and the title rule answers. A version of
   `tmux` that replaces that frame inside the pane keeps the screen rule: such a
   Codex pane reports Working and no Needs you, because a title that holds no
   state is no claim.
4. The text is POSIX shell and its tests run it under each shell that the test
   computer has. macOS has `sh` and `bash` by default and no `dash` and no
   `BusyBox`, so those two lanes run on Linux only. This computer needs the
   OpenSSH client `ssh` in its `PATH`.

The sidebar shows these agents under each machine of the list, with the state
that a local agent panel shows, and a press on a row opens the terminal of that
`tmux` session. Settings, General, Machines holds the check box that turns the
watch off. The click needs the OpenSSH client, which both systems have.

## The keyboard

The Control key of a Mac is the control key of the terminal. **Control+C**
interrupts the program in the panel. The Cmd key of a Mac is the command key of
the window, and it never writes a control character into a terminal. A panel
writes no byte for a key that the person presses with Cmd and without Control.
**Cmd+C** copies the selection of a terminal and writes nothing, **Cmd+V**
pastes, and **Cmd+A** writes nothing. On Linux the Control key is both the
control key of the terminal and the command key of the window, so **Ctrl+C**
interrupts the program and the toolkit reports the same key as Cmd
(`crates/horizon-ui/src/input/keyboard.rs`).

The Option key of a Mac is the Alt key. It sends Meta, which is `ESC` and the
key, when the layout composes no character for it. Some layouts compose a
character: a Norwegian layout gives `|` for Option+7. The terminal then receives
that character alone, which is the character that the person typed. On Linux the
Alt key sends Meta and AltGr writes the character that the layout composes.

A shortcut of a key position follows that position on every layout. On a
Norwegian layout Shift+comma composes `;`, and **Cmd+Shift+Comma** still opens
Settings (`crates/horizon-ui/src/app/shortcuts.rs`).

## Install and update

| Function | Linux | macOS |
|---|---|---|
| Release binary (note 1) | x64 | arm64 and x64 |
| App bundle with the Dock icon (note 3) | n/a | Partial |
| Homebrew | x64 | Yes |
| Code signing and notarization in the release | n/a | No (note 2) |

1. The release assets and the Homebrew tap come from the release workflow. See
   [the release flow](release-flow.md).
2. The release workflow and the packaging scripts have no signing or
   notarization step.
3. The macOS asset is a bare binary. `packaging/macos/make_app_bundle.sh` makes
   `Horizon.app` from a release build with the icon, the bundle name and the
   `Info.plist`, and `--install` copies it to the applications folder. The
   `Info.plist` holds `NSMicrophoneUsageDescription`, the reason that macOS
   shows for the microphone that speech input uses, and the check
   `scripts/test_make_app_bundle.py` fails when that key is absent.
4. A window that a person starts from the Finder inherits the `PATH` of
   launchd, `/usr/bin:/bin:/usr/sbin:/sbin`, and not the `PATH` of a terminal
   shell. Horizon adds `/opt/homebrew/bin`, `/usr/local/bin`, `~/.local/bin` and
   `~/.cargo/bin` after the `PATH` of this process on macOS, when a folder exists
   and is not there yet, for the programs that it resolves itself
   (`crates/horizon-browser/src/process.rs:463-496`). A shell panel and an agent
   panel start through the shell of the person, so they already find those
   programs. A command panel still starts its program with the `PATH` of the
   process.

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
| The hotkey, the microphone icon of the title bar, the pill and the insert of the text | Yes | Yes |
| The default recorder line | Yes: `pw-record` of PipeWire records 16 kHz on one channel | No: no recorder is a part of macOS, so the line is empty and the person installs one |
| A recorder recipe that the documents give | `pw-record --rate 16000 --channels 1 --format s16 {file}` | `ffmpeg -f avfoundation -i :default -ac 1 -ar 16000 -y {file}`, or `rec -c 1 -r 16000 {file}` of SoX |
| A take with a real microphone | Yes with PipeWire | Not tested: no run of this repository used a real microphone on macOS |
| The transcription service over HTTPS | Yes | Yes |
| The transcription command | Yes | Yes |

The microphone icon in the title bar of a panel states the take of that panel:
it is red while the take records and it pulses amber while the take is
transcribed. One click on the icon starts a take into that panel, and one click
on the red icon stops it. The pill at the bottom of the panel holds the
sentences only: the reason of a failed take, and the offer of a kept take.

A Linux computer needs a PipeWire session, because `pw-record` records from it.
macOS holds no recorder that a person can use without an install, so the tab says
which two programs work and the exact line of each. Settings › Speech names the
same lines, and [the speech input procedure](testing/procedures/speech-input.md)
has a task for each.
The stand-in recorder of the fixture,
`scripts/device-smoke/speech_standin_recorder.py`, copies one prepared file and
opens no audio device, so a test needs no microphone on either platform. See
[the speech input procedure](testing/procedures/speech-input.md).

## Agent skills

Horizon installs the `horizon-browser`, `horizon-device`, `horizon-mobile` and
`horizon-speech` skills on both platforms. The `horizon-speech` skill says what a
take needs on each platform, the recorder recipes, the named secret of the key,
and what leaves the computer. The `horizon-mobile` skill drives an app on a real
phone of a device cloud, and its tool is one Python script that runs on both
platforms. The `horizon-device` skill tells agents to drive an
isolated desktop. This part works on Linux with X11 only (note 2 under
"Panels and the board").
