# Device panel over an SSH tunnel — smoke test plan

Validates that a native Device panel can reach a VNC server that only listens
on a remote host's loopback interface, by running `ssh -W <target>` and piping
that process's stdio straight into the VNC decoder. The tunnel process must live
exactly as long as the viewer connection, and ssh's own diagnostics must reach
the panel when the forward fails.

Public evidence must use only synthetic fixtures: a user-level `sshd` on a high
port and an Xvfb display served by `x11vnc -localhost`. Do not tunnel to a
developer's real hosts.

## Lane A — unit (no live viewer)

```sh
cargo test -p horizon-core device::
cargo test -p horizon-core ssh::
cargo test -p horizon-ui --bin horizon device_widget
```

Must include:

- `stdio_forward_args_relay_the_remote_endpoint_in_batch_mode` — `ssh -W`
  arguments carry the transport options, accept a first-contact host key
  (batch mode cannot prompt) and never the remote command.
- `tunnelled_device_keeps_its_ssh_host_across_restore` — the tunnel host is
  persisted with the panel and restored without reconnecting.
- `tunnel_without_a_host_is_rejected_before_panel_creation` — an empty SSH
  host fails before a panel exists.
- `tunnel_relays_bytes_through_the_program_stdio_and_reaps_it_on_drop` and
  `a_tunnel_dropped_inside_a_cancelled_future_is_still_reaped` — the forward
  is bidirectional and the child is killed and reaped (no zombie entry) when
  the session ends, including when its future is cancelled.
- `a_failed_tunnel_reports_the_cause_and_summary_lines` and
  `ssh_prefixed_diagnostics_are_not_prefixed_twice` — ssh's last two stderr
  lines are appended to the viewer error exactly once.

Status: **PASS** (2026-09-23, Linux x64; 15 core device tests, 36 device
widget tests).

## Lane B — live tunnel on an isolated desktop (Linux)

Fixture setup, all task-owned and outside the developer's `~/.ssh`:

1. Extract `openssh-server` and `x11vnc` from the apt cache into a scratch root
   (`apt-get download …; dpkg-deb -x …`). Generate a host key and a client key
   pair under the scratch root, write an `sshd_config` with `Port 2299`,
   `ListenAddress 127.0.0.1`, `UsePAM no`, `PasswordAuthentication no`,
   `StrictModes no`, `AllowTcpForwarding yes`, and start it with the absolute
   binary path. Write a client config with a `Host smoke-node` alias pointing at
   `127.0.0.1:2299` and the scratch identity.
2. `Xvfb :97 -screen 0 1024x700x24`, then
   `x11vnc -display :97 -localhost -viewonly -forever -shared -rfbport 5997 -nopw`.
3. Prove the forward independently:
   `(sleep 2) | ssh -F <client_config> -o BatchMode=yes -W 127.0.0.1:5997 smoke-node | head -c 12`
   must print `RFB 003.008`.
4. Launch the candidate `horizon --config tunnel.yaml --ephemeral` on a second
   Xvfb display with openbox running. The config declares three `kind: device`
   panels whose `ssh_connection` uses `extra_args: ["-F", "<client_config>"]`:
   `127.0.0.1:5997` via `smoke-node`, `127.0.0.1:5997` via `nowhere.invalid`, and
   `127.0.0.1:5998` (no listener) via `smoke-node`.

Checks:

### B1. Route is visible before connecting

Each panel header shows `<target> via <host>` and the subtitle
`VNC desktop over SSH`; status is Stopped because config-restored Device panels
never connect on their own.

### B2. Good tunnel connects and renders

Click **Reconnect** on the `smoke-node` panel. Within a few seconds the status
is Connected, the VNC name is the Xvfb display name, the desktop image renders,
and exactly one `ssh -W 127.0.0.1:5997 … smoke-node` child of the Horizon
process exists.

### B3. Failures carry ssh's diagnostic

Click **Reconnect** on the other two panels. The unknown-host panel reports
`… ssh: Could not resolve hostname nowhere.invalid …`; the closed-port panel
reports `… ssh: channel 0: open failed: connect failed: Connection refused;
stdio forwarding failed`. Their ssh children exit on their own.

### B4. Closing the panel reaps the tunnel

Close the connected panel from the sidebar. The Horizon process has no
`ssh -W` children afterwards.

Status: **PASS** (2026-09-23, Linux x64, Xvfb `:98` + openbox, debug build of
this branch). B1 to B4 observed as described; the closed-port panel showed
`early eof; ssh: channel 0: open failed: connect failed: Connection refused;
stdio forwarding failed`, and the Horizon process had no `ssh -W` child after
each failed forward and after the connected panel was closed.

## Not covered

- macOS and Windows use the same `ssh` binary path and stdio piping; only Linux
  was run live. The unit tests in Lane A are platform independent except the
  three `sh`/`cat` based tunnel tests, which are Unix only.
- Password-protected VNC servers still fail explicitly; the viewer forwards no
  credentials and no input.
