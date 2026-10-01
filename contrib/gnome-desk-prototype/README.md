# Desktop-workspace prototype (GNOME on Wayland)

A throwaway prototype of Horizon without its own window chrome. It exists to prove the idea and to be
rebuilt properly later; none of it is meant to ship as is.

What it does, with `HORIZON_DESK_MODE=1`:

- every Horizon workspace opens as its own native window (a detached workspace) with no title bar or toolbar,
  and a GNOME Shell extension puts each one on its own desktop workspace;
- the root window becomes a command bar that is always on top and visible on every desktop workspace;
- the bar shows a minimap of the workspaces (what runs in each, and whether an agent is working or needs you).
  Clicking a tile switches desktop, and the corner dot of a tile narrows the assistant to that workspace;
- the bar expands into one of three layouts (A Sheet, B Split, C Stage) for the whole conversation;
- the assistant is not tied to a workspace: it reaches all of them unless its scope is narrowed.

## Pieces

| Path | What it is |
|:-----|:-----------|
| `extension/horizon-desk@horizon` | GNOME Shell 50 extension exporting `dev.horizon.Desk` on the session bus: `State`, `EnsureWorkspaces`, `MoveWindow`, `StickWindow`, `KeepAbove`, `Place`, `Switch`. |
| `crates/horizon-ui/src/app/desk.rs` | Worker thread that calls the extension through `gdbus`. |
| `crates/horizon-ui/src/app/assistant/summon/desk_bar.rs` | The command bar window, minimap tiles, scope picker and the three expanded layouts. |
| `crates/horizon-ui/src/app/assistant/summon/demo.rs` | Scripted input for recordings (`HORIZON_DESK_SCRIPT`). |
| `headless-gnome.sh`, `run-horizon.sh` | A private headless GNOME Shell and a Horizon started inside it. |
| `demo/` | Stand-in agents, the scripted story, the frame recorder. |
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

Requirements that were true on Ubuntu 26.04 (GNOME Shell 50.1): `gnome-shell --headless --virtual-monitor`,
`--unsafe-mode` for the screenshot and `Eval` interfaces, and `gdbus`.

## Known limits

- Linux with GNOME only. Wayland gives applications no way to place windows on a workspace, so this needs the
  extension. X11 would use the standard window-manager desktop property instead; macOS has no public API for it.
- The extension is matched to windows by title (`<workspace name> · Horizon`), which is fine for a prototype.
- The agents in the demo are scripts that print progress; the voice is synthesized. No model is called.
- The window of the bar is rectangular: requesting transparency did not take effect in the headless session.
