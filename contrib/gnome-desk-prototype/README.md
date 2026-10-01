# Desktop-workspace prototype (GNOME on Wayland)

A throwaway prototype of Horizon without its own window chrome. It exists to prove the idea and to be
rebuilt properly later; none of it is meant to ship as is.

What it does, with `HORIZON_DESK_MODE=1`:

- **Every panel is a native window** of its own, with no Horizon canvas or frame around it. The desktop draws the
  title bar and moves, resizes and tiles it like any other application. Terminals, agents, browser panels and
  Device (native VNC viewer) panels all work this way.
- **Every Horizon workspace is a desktop workspace.** A GNOME Shell extension puts each panel window on the desktop
  of its workspace. Drag a window to another desktop (or press Super+Shift+Page Up/Down) and the panel follows
  into that Horizon workspace.
- **The root window becomes a command bar** that is always on top and visible on every desktop workspace. It
  shows a minimap of the desktops built from the real windows on each (agent state per tile, click to switch).
  With many workspaces the tiles shrink into one compact row; the bar stays small.
- **The bar expands** into one of three layouts (A Sheet, B Split, C Stage) for the whole conversation.
- **The assistant belongs to no workspace**: it reaches all of them unless its scope is narrowed in the chip's picker.
- GNOME's own shortcuts (Ctrl+Alt+Left/Right) and Overview keep working; the minimap follows them.
- **Windows are marked as Horizon's.** The extension draws a thin accent outline and a small "Horizon" tag in the
  empty left of the title bar of every window whose app id starts with `horizon-panel-`. The windows themselves
  are untouched, so a panel is recognisable at a glance among other applications.
- **The conversation is a feed, not a terminal**: bubbles for what was said, pills for what the assistant did,
  the plan as a progress bar with a chip per step, a card per agent, and a card with Allow / Deny when an agent
  asks the person something (the answer is typed into that agent as the person would). The raw terminal is one
  click away.
- **Mini mode** shrinks the bar to rest just above the dock (the extension reports the work area, so placement
  follows the dock). Three designs, switched live (right-click the mark): Pill, Strip and Orb.
- **Quick nav, remote hosts, cloud, sessions and settings** have a row of buttons under the prompt. What opens is
  also shown three ways: A, a native window per page with an arrow to its button; B, one hub window with tabs;
  C, inline in the bar (right-click a button to switch). Hosts, sessions and quick nav show real data; cloud
  shows sample environments.

## Pieces

| Path | What it is |
|:-----|:-----------|
| `extension/horizon-desk@horizon` | GNOME Shell 50 extension exporting `dev.horizon.Desk` on the session bus: `State`, `EnsureWorkspaces`, `MoveWindow`, `StickWindow`, `KeepAbove`, `Place`, `Switch`. |
| `crates/horizon-ui/src/app/desk.rs` | Worker thread that calls the extension through `gdbus`. |
| `crates/horizon-ui/src/app/panels/window.rs` | Renders one panel as the whole content of a window. |
| `crates/horizon-ui/src/app/assistant/summon/desk_windows.rs` | Opens a window per panel, puts it on its desktop, and follows the person's moves. |
| `crates/horizon-ui/src/app/assistant/summon/desk_bar.rs` | The command bar window, minimap tiles, scope picker and the three expanded layouts. |
| `crates/horizon-ui/src/app/assistant/summon/demo.rs` | Scripted input for recordings (`HORIZON_DESK_SCRIPT`). |
| `headless-gnome.sh`, `run-horizon.sh` | A private headless GNOME Shell and a Horizon started inside it. |
| `demo/` | Stand-in agents, a 20-workspace config generator, a VNC fixture for the Device panel, the scripted story, the frame recorder. |
| `video/` | Event timing, synthesized music and sounds, and the composer for the marketing cut. |

## Run it without touching your own desktop

Everything runs in a private headless session; the live desktop is never used.

```bash
export DEMO_DIR=$HOME/horizon-desk-demo && mkdir -p $DEMO_DIR/gnd/{rt,home}
chmod 700 $DEMO_DIR/gnd/rt
# 1. install the extension into the private home and enable it
mkdir -p $DEMO_DIR/gnd/home/.local/share/gnome-shell/extensions
cp -r extension/horizon-desk@horizon $DEMO_DIR/gnd/home/.local/share/gnome-shell/extensions/
# 2. start the private shell (virtual 1600x1000 monitor), then enable the extension through its session bus
./headless-gnome.sh &
```

The shell must be restarted after the extension is first enabled
(`gsettings set org.gnome.shell enabled-extensions "['horizon-desk@horizon']"` with the private session bus
address, which is the one in `/proc/<gnome-shell pid>/environ`). Then run `run-horizon.sh`; the scripted
story is replayed by `demo/take.sh`, which records frames of the private desktop with the shell's
`org.gnome.Shell.Screenshot` call (about 20 frames per second).

Requirements that were true on Ubuntu 26.04 (GNOME Shell 50.1): `gnome-shell --headless --virtual-monitor 1920x1080`,
`--unsafe-mode` for the screenshot interface, and `gdbus`. The demo also uses `Xvfb`, `openbox`, `x11vnc` and
`gnome-calculator` (the native app in the VNC viewer), a Chromium binary, and Python 3 with `websockets` for the voice agent. `demo/up.sh` also enables the Ubuntu dock at the bottom so mini mode has something to rest on.

## Known limits

- Linux with GNOME only. Wayland gives applications no way to place windows on a workspace, so this needs the
  extension. X11 would use the standard window-manager desktop property instead; macOS has no public API for it.
- The extension is matched to windows by title (`<workspace name> · Horizon`), which is fine for a prototype.
- The coding agents in the demo are scripts that print progress. The assistant in the recorded take is a live
  OpenAI Realtime voice agent (`demo/voice_agent.py`) that is told which MCP tools it may use and decides itself
  what to call; the person's spoken request is a recording generated with OpenAI text-to-speech
  (`demo/gen_request.py`) and streamed to it in real time. A harness nudge asks it to carry on if it stops before
  posting the recap. Needs an API key file (`OPENAI_KEY_FILE`); the key is never part of the repo or the video.
- Browser engines other than Chromium (Firefox, Safari) and remote BrowserStack sessions use the same panel model but were not exercised in the demo: they need a driver or an account. Cloud environments need a provider account, so the demo's "Cloud" workspace is a local stand-in.
- On GNOME 50 the shell's workspace switcher handler takes an extra event argument; the extension handles both.
- The three mini designs and the three hub designs are for comparison and will be reduced to one each.
