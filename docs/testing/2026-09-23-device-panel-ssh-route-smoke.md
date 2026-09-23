# device_panel SSH route — smoke test plan

Validates that an agent can create a tunnelled native Device viewer through the
public `device_panel` MCP tool: `create` takes an optional `ssh` route
(`host`, `user?`, `port?`), Horizon reaches `endpoint` on that host's loopback
through `ssh -W` with its own SSH configuration and keys, and `list`/`inspect`
report the route. This is the MCP counterpart of the Remote Hosts overlay's
VNC mode (`2026-09-23-device-ssh-tunnel-smoke.md`).

Public evidence must use only the synthetic fixtures from
`2026-09-23-device-ssh-tunnel-smoke.md` (user-level `sshd` on
`127.0.0.1:2299`, `x11vnc` on `127.0.0.1:5997`).

## Lane A — unit (no live viewer)

```sh
cargo test -p horizon-browser-control device
cargo test -p horizon-ui --bin horizon device_requests
```

Must include:

- `create_accepts_an_ssh_route_and_older_requests_without_one` — the request
  schema stays backward compatible; `identity_file` or any other key inside
  `ssh` is rejected.
- `ssh_routes_are_trimmed_and_option_like_or_broken_labels_are_refused` —
  blank labels, a leading `-`, whitespace, control characters and port zero.
- `create_with_an_ssh_route_tunnels_the_viewer_and_reports_the_route` — the
  created panel carries only host, user and port in its tunnel connection,
  persists it like an SSH panel, `list` reports `ssh`, and a refused route
  (`invalid_ssh_route`) creates nothing.

Status: **PASS** (2026-09-23, Linux x64; 7 control-crate and 17 host tests).

## Lane B — live MCP on an isolated desktop (Linux)

Setup: Xvfb `:98` + openbox, private `HOME` whose `.ssh/config` has a
`Host smoke-node` block (HostName 127.0.0.1, Port 2299, the fixture key, its
own `UserKnownHostsFile`), a config with one `Ops` workspace holding a
`kind: codex` panel whose `command`/`args` run a Python probe that writes
`HORIZON_BROWSER_ACTOR` and `HORIZON_BROWSER_HOST_INSTANCE` to a file. Launch
the candidate with `--config <yaml> --ephemeral`. OpenSSH resolves `~` from the
passwd entry rather than `$HOME`, so run Horizon under `nss_wrapper`
(`LD_PRELOAD=libnss_wrapper.so NSS_WRAPPER_PASSWD=<file>` naming the private
HOME) so `ssh` reads that `.ssh/config`; without it the tunnel fails with
`Could not resolve hostname smoke-node`. Drive `horizon --browser-mcp` with the
probe's identity through `scripts/browser-smoke/mcp_gate.py`'s `McpClient`.

### B1. Create through the route

`device_panel` `{operation: create, endpoint: "127.0.0.1:5997", ssh: {host: "smoke-node"}}`
returns `status: panels` with one panel whose `endpoint` is the host's
loopback address and whose `ssh` is `{host: "smoke-node"}`; a `Device
smoke-node` panel appears in `Ops` and Horizon spawns `ssh -W 127.0.0.1:5997`.

### B2. Live image

Within a few seconds `list` reports `connection: connected`,
`image_received`, `image_displayed`, an advancing `frame_sequence`, and the
server name and desktop size from the VNC handshake, with `ssh` still reported.

### B3. Option-like host is refused

`create` with `ssh: {host: "-oProxyCommand=id"}` returns
`status: failed, code: invalid_ssh_route` and creates no panel.

### B4. Close releases the tunnel

`close` returns `status: closed`; no `ssh -W` child of Horizon remains.

Status: **PASS** (2026-09-23, Linux x64, debug build of this branch). B1: the
panel reported `endpoint 127.0.0.1:5997`, `ssh {host: smoke-node}`,
`presentation: connecting`, and Horizon's child list showed `ssh -W
127.0.0.1:5997 -o BatchMode=yes …`. B2: three inspections 4 s apart read
`connected`, `image_received: true`, `image_displayed: true`,
`frame_sequence: 5`, server `peters:97` at 1024×700. B3: `invalid_ssh_route`,
"ssh.host must be a plain host label without options or spaces". B4: `closed`,
zero `ssh -W` children afterwards.

## Not covered

- macOS and Windows use the same code; only Linux was run live.
- A route to a host that is not yet in `known_hosts` fails under BatchMode
  with ssh's own message (covered by the tunnel smoke), not exercised here.
