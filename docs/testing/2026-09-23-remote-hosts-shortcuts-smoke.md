# Remote Hosts row menu and saved shortcuts — smoke test plan

Validates the host row's context menu in the Remote Hosts overlay: opening a
host over SSH or VNC from the menu, and saving the host as a preset that the
command palette can create in any workspace.

Public evidence must use only the synthetic fixtures from
`2026-09-23-device-ssh-tunnel-smoke.md` and
`2026-09-23-remote-hosts-vnc-workspace-smoke.md`.

## Lane A — unit (no live viewer)

```sh
cargo test -p horizon-ui --bin horizon remote_hosts
```

Must include:

- `row_menu_lists_open_and_save_for_both_modes` — the four menu items.
- `the_row_menu_saves_a_shortcut_with_the_user_override_or_opens_in_the_header_destination`
  — a menu open uses the menu's mode and the header's workspace; a save
  carries the `user@` override.
- `a_right_click_menu_above_the_card_saves_a_shortcut` — egui-driven: a
  right click (press and release in one frame) selects the row and opens the
  menu above the card; picking "Save VNC shortcut" yields the action and
  closes the menu.
- `a_notice_replaces_the_host_count_until_it_expires` — the header feedback.
- `saving_shortcuts_stores_presets_the_palette_can_create_anywhere` — presets
  `VNC: <host>` (Device, tunnel target from `remote_hosts.vnc_port`) and
  `SSH: <host>` are written to the config, applied live, refused for a blank
  host, and replaced rather than duplicated when saved again.

Status: **PASS** (2026-09-23, Linux x64; 23 remote hosts tests).

## Lane B — live overlay on an isolated desktop (Linux)

Setup as in the workspace smoke (fixture `sshd` + `x11vnc`, private `HOME`
with a `Host smoke-node` block, `remote_hosts.vnc_port: 5997`, one `Ops`
workspace), launched with `--config overlay.yaml --ephemeral`.

### B1. Menu above the card

Press **Ctrl+Shift+H**, type `smoke`, right-click the row. The row is
selected and a menu with Open over SSH, Open over VNC, Save SSH shortcut and
Save VNC shortcut appears above the card.

### B2. Save VNC shortcut

Click **Save VNC shortcut**. The menu closes, the header shows
`Saved preset "VNC: smoke-node"` for a few seconds, and the loaded config file
gains a preset `VNC: smoke-node` of kind `device` with `command: 127.0.0.1:5997`
and the host's `ssh_connection`.

### B3. Create it anywhere

Press **Escape**, focus `Ops`, open the command palette (**Ctrl+Shift+K**),
type `VNC: smoke` and press **Enter**. A tunnelled Device panel opens in
`Ops` and connects to the fixture desktop.

### B4. Open from the menu

Open the overlay again, right-click the row and pick **Open over SSH**. An
SSH terminal panel opens in the header's destination workspace.

Status: **PASS** (2026-09-23, Linux x64, Xvfb `:98` + openbox, debug build of
this branch). B1 to B4 observed as described: the menu drew above the card
with the row selected, the header read `Saved preset "VNC: smoke-node"`, the
config gained the `device` preset with `command: 127.0.0.1:5997` and the
host's connection, the palette entry created a connected tunnelled panel in
`Ops`, and Open over SSH put a connected terminal in `Remote Sessions`.

## Not covered

- macOS and Windows use the same code; only Linux was run live.
