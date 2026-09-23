# Remote Hosts per-host VNC ports — smoke test plan

Validates that a host's VNC port can differ from `remote_hosts.vnc_port`:
through the `remote_hosts.vnc_ports` map (keyed by the overlay label or the
SSH host name) and through a `:port` suffix typed after the overlay filter.
Resolution order is typed port, then the map (label before host name), then
the global port, for both opening a host and saving a VNC shortcut.

Public evidence must use only the synthetic fixtures from
`2026-09-23-device-ssh-tunnel-smoke.md` and
`2026-09-23-remote-hosts-vnc-workspace-smoke.md`.

## Lane A — unit (no live viewer)

```sh
cargo test -p horizon-core remote_hosts
cargo test -p horizon-ui --bin horizon remote_host
```

Must include:

- `per_host_ports_resolve_override_then_label_then_host_then_global` and
  `blank_workspace_and_port_zero_are_rejected` — the config's resolution
  order, exact keys, and validation of blank keys and port zero.
- `section_round_trips_through_yaml` — an empty map is not written, a
  populated one round-trips.
- `parse_query_takes_a_port_suffix_only_when_it_is_the_sole_colon` — `:port`
  parsing: digits only, 1 to 65535, and never inside an IPv6 filter.
- `enter_opens_the_selected_host_with_the_current_mode_and_destination` and
  `the_row_menu_saves_a_shortcut_with_the_user_override_or_opens_in_the_header_destination`
  — the typed port travels with both the open and the save actions.
- `per_host_ports_and_a_typed_port_override_the_global_vnc_port` — the
  Device panel target for label, host-name, typed and unlisted hosts.
- `saving_shortcuts_stores_presets_the_palette_can_create_anywhere` — a
  re-saved VNC shortcut freezes the typed port into the preset command.

Status: **PASS** (2026-09-23, Linux x64; 8 core `remote_hosts` tests, 40 UI
`remote_host` tests, full workspace and speech test tiers green).

## Lane B — live overlay on an isolated desktop (Linux)

Setup as in the workspace smoke (fixture `sshd` on `127.0.0.1:2299`,
`x11vnc` on `127.0.0.1:5997`, private `HOME` with a `Host smoke-node` block,
one `Ops` workspace), launched with `--config <yaml> --ephemeral`. Nothing
listens on the global port in either config, so a connected desktop proves
the per-host port was used.

### B1. Map entry by overlay label

Config: `remote_hosts: { vnc_port: 5900, vnc_ports: { smoke-node: 5997 } }`.
Press **Ctrl+Shift+H**, type `smoke`, **Tab** to VNC, **Enter**. A Device
panel opens in `Remote Sessions` and shows the fixture desktop; the panel's
details name `127.0.0.1:5997` through the tunnel.

### B2. Typed `:port` with no map entry

Config: `remote_hosts: { vnc_port: 5900 }`. Press **Ctrl+Shift+H**, type
`smoke:5997`. The row still matches (the suffix is not part of the filter),
the empty-filter hint reads `type to filter · user@ sets the user · :port
the VNC port`. **Tab**, **Enter**: the panel connects to the fixture desktop
on 5997.

### B3. Typed `:port` on a saved shortcut

With `smoke:5997` still typed, right-click the row and pick **Save VNC
shortcut**. The config file gains `VNC: smoke-node` with
`command: 127.0.0.1:5997`.

Status: **PASS** (2026-09-23, Linux x64, Xvfb `:98` + openbox, debug build of
this branch). B1: with the global port at 5900 (nothing listening) and
`vnc_ports: { smoke-node: 5997 }`, Enter spawned `ssh -W 127.0.0.1:5997` and
the panel read `127.0.0.1:5997 via peters@127.0.0.1`, Connected. B2: with no
map, `smoke:5997` kept the row listed (`1/901`), the hint fit the header at
1400 px, and Enter connected through `ssh -W 127.0.0.1:5997`. B3: Save VNC
shortcut showed `Saved preset "VNC: smoke-node"` and the loaded file gained
the `device` preset with `command: 127.0.0.1:5997` and the host's
`ssh_connection`.

## Not covered

- macOS and Windows use the same code; only Linux was run live.
