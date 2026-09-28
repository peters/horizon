# Local Network Bridge card smoke (temporary)

Temporary validation plan for the **Share local network** switch (#987 M0). Delete
it before the PR merges. Follow `AGENTS.md` "Isolated UI Testing Through Horizon
Native VNC": run the candidate on a task-owned Xvfb display through
`scripts/device-smoke/serve.py --native-view`, view it live in a native Device
panel in the calling agent's workspace, record the test display, and keep private
evidence out of the PR.

## Setup

1. Build and freeze the candidate: `cargo build -p horizon-ui --bin horizon`, then
   copy `target/debug/horizon` into a private directory and record its SHA-256.
2. Worker: a Ready cloud whose image includes this branch's `horizon-cloud-worker`.
   Record the provider, worker size, image digest and the cleanup deadline for a
   paid worker. A local worker container can stand in for the bridge checks, not for
   the card, because the card needs a Ready production cloud.
3. This computer must be on an IPv4 network (`/16` to `/30`) through a non
   point-to-point default route. Record the subnet. Pick one device on it with a TCP
   service (for example the router's web page on port 80) and one address outside it.
4. Launch the fixture with a private state directory, create the Device panel from
   its `vnc_address`, and confirm `connection`, `image_received`, `image_displayed`
   and an advancing `frame_sequence`.
5. Start a display-scoped recording before step A1.

## A. Card

| Step | Action | Expected |
|------|--------|----------|
| A1 | Open the Ready cloud's card | **Share local network** is present and off; no status line |
| A2 | Hover the switch | Tooltip says TCP, relayed through this computer, off after restart, every worker process can use it |
| A3 | Switch it on | "Connecting to share <subnet>…", then "Sharing <subnet> · 0 open · 0 B" within about 10 s |
| A4 | Run B1 to B3 on the worker | The line's open count and bytes change within about a second |
| A5 | Switch it off | The status line disappears at once; the UI does not stall |
| A6 | Switch it on again | A new "Sharing" line; the worker reports a new proxy port |
| A7 | Resize the window, then Fit | Card text stays inside the card; screenshot after launch and after resize/Fit |
| A8 | Disconnect the cloud (stop the worker or break its SSH) | The switch turns off by itself when the card leaves Ready |
| A9 | Quit and relaunch Horizon with the same state | The switch is off; nothing starts on its own |
| A10 | Switch it on while this computer's default route is a VPN point-to-point link, or with no network | The switch stays off and the card explains why |
| A11 | Worker image without the helper (older image) | The card says to rebuild the image; it does not keep retrying |

## B. Worker side (shell panel or SSH on the worker)

| Step | Command | Expected |
|------|---------|----------|
| B1 | `horizon-cloud-worker local-network status` | `active: true`, the subnet, a `127.0.0.1:<port>` proxy |
| B2 | `curl --socks5-hostname <proxy> http://<device>/` | The device answers |
| B3 | `horizon-cloud-worker local-network forward <device> 80`, then `curl http://127.0.0.1:<worker_port>/` | The device answers |
| B4 | `forward <address outside the subnet> 80` | "Outside the bridged local network…" |
| B5 | `forward <this computer's address> 22` and `curl --socks5-hostname <proxy> http://localhost:8080/` | Refused; this computer is never reachable |
| B6 | `forward <device> <closed port>` | "The device refused the connection on that port" |
| B7 | `unforward <worker_port>` | The forward is gone from `status` |
| B8 | After A5 | `status` reports `active: false` with the owner-only note; `/run/horizon-local-network/` holds only `lock` |
| B9 | An agent's MCP tools list | `local_network_status`, `local_network_forward`, `local_network_unforward` |

## Evidence

Keep the frozen binary hash, the actual Horizon child PID, the Device panel
observations with timestamps, screenshots after launch and after resize/Fit, and the
recording with decoded frames from A3 to A5. Publish only Horizon screenshots with
the subnet, addresses and worker identifiers redacted.
