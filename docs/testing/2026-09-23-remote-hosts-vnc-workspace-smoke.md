# Remote Hosts overlay: SSH or VNC into a chosen workspace — smoke test plan

Validates the overlay's **SSH | VNC** switch, the destination workspace picker,
and the persisted default workspace on top of the SSH-tunnelled Device panel
from `2026-09-23-device-ssh-tunnel-smoke.md`.

Public evidence must use only synthetic fixtures: the user-level `sshd` and
`x11vnc -localhost` fixture from the tunnel smoke, reached through an isolated
`~/.ssh/config`. Do not open the developer's real hosts.

## Lane A — unit (no live viewer)

```sh
cargo test -p horizon-core config::
cargo test -p horizon-ui --bin horizon remote_hosts
```

Must include:

- `defaults_name_the_remote_sessions_workspace_and_the_standard_vnc_port`,
  `missing_section_and_partial_sections_fill_in_defaults`,
  `blank_workspace_and_port_zero_are_rejected`, `section_round_trips_through_yaml`
  — the `remote_hosts` config section.
- `enter_opens_the_selected_host_with_the_current_mode_and_destination`,
  `tab_toggles_between_ssh_and_vnc` — overlay state.
- `default_entry_comes_first_and_marks_a_missing_workspace_as_new`,
  `an_existing_default_workspace_is_listed_once_through_the_default_entry`,
  `a_closed_workspace_selection_falls_back_to_the_default` — picker entries.
- `vnc_opens_a_tunnelled_device_panel_in_the_default_workspace`,
  `the_configured_vnc_port_and_workspace_name_are_used`,
  `ssh_opens_in_the_chosen_existing_workspace_and_reuses_the_default_one`,
  `setting_the_default_workspace_rewrites_the_config_and_applies_it` — the app
  side, including the config rewrite.
- `destination_picker_opens_above_the_card_and_selects_a_workspace` and
  `picker_stays_above_the_card_after_the_overlay_was_dismissed_with_it_open` —
  egui-driven: the picker popup is drawn above the card even when a press and
  release arrive in one frame after an earlier overlay was dismissed with the
  popup open (the case a slow display produced live; the popup is registered as
  a sublayer of the card for that reason).
- `destination_picker_opens_with_workspaces_and_panels_on_the_board` — the same
  click through the whole app frame with a sidebar and panels present.

Status: **PASS** (2026-09-23, Linux x64; 42 config tests, 18 remote hosts
tests, 1 full-app test).

## Lane B — live overlay on an isolated desktop (Linux)

Setup:

1. Keep the tunnel smoke fixture running (`sshd` on 127.0.0.1:2299, Xvfb `:97`
   served by `x11vnc -localhost -rfbport 5997`).
2. Use a private `HOME` whose `.ssh/config` has one `Host smoke-node` block
   (HostName 127.0.0.1, Port 2299, the fixture identity, a private
   `UserKnownHostsFile`). Discovery lists it beside any Tailscale peers of the
   machine; filter on `smoke` so only the fixture row is visible in evidence.
3. Launch the candidate `horizon --config overlay.yaml --ephemeral` on a second
   Xvfb display with openbox. The config has `remote_hosts.vnc_port: 5997` and
   one workspace named `Ops` with an editor panel.

Checks:

### B1. Switch and picker

Press **Ctrl+Shift+H**. The header shows `Remote  [SSH][VNC]  >`, the filter has
focus, and the right side reads `in  Remote Sessions (new) ▾`. Type `smoke`:
one row remains. Press **Tab**: VNC is highlighted and the filter still has
focus (typing continues to filter).

### B2. VNC into the default workspace

Press **Enter**. The overlay closes, a `Remote Sessions` workspace appears
laid out as a grid, and it holds a Device panel titled `smoke-node` whose
header reads `127.0.0.1:5997 via <user>@127.0.0.1` / `VNC desktop over SSH`.
It connects on its own and renders the fixture desktop.

### B3. SSH into a picked workspace

Open the overlay again, filter `smoke`, click the picker, choose `Ops`, and
press **Enter** (mode is SSH again because the overlay starts fresh). An SSH
terminal panel `smoke-node` opens in `Ops`, not in `Remote Sessions`.

### B4. Set default

Open the overlay, pick `Ops` in the picker, click **Set default**. The
button disappears, the picker reads `Ops`, and the loaded config file now
contains `remote_hosts: { default_workspace: Ops, vnc_port: 5997 }` while
`Remote Sessions` keeps its panel. Press **Escape** to close.

Status: **PASS** (2026-09-23, Linux x64, Xvfb `:98` + openbox at 4 to 12 fps,
debug build of this branch). B1 to B4 observed as described: Tab kept the
filter caret, the VNC panel connected with `127.0.0.1:5997 via <user>@127.0.0.1`
and one `ssh -W` child, the SSH panel landed in `Ops`, and Set default logged
`config updated setting="remote_hosts.default_workspace"` and rewrote the
file. The first live run found the picker popup drawn beneath the card on a
second open; that is the sublayer fix covered by Lane A.

## Not covered

- Per-host VNC ports: the port is one config value for all hosts.
- macOS and Windows use the same overlay code; only Linux was run live.
