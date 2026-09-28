# Local Network Bridge Scope Editor Smoke Plan

Use this checklist to check the card's **Scope** editor on a real cloud (#987 M2).

## Goal

With **Share local network** on, the owner narrows the bridge to one device and
port, opens one port on this computer, and widens it again. Each change applies
at once. An agent on the worker reaches a dev server on this computer only after
its port is opened.

## Setup

- The same setup as the
  [open connections smoke](2026-09-28-local-network-bridge-connections-smoke.md):
  - a frozen candidate of the PR head on a task-owned isolated desktop, viewed live
    in a Horizon native VNC Device panel;
  - a recorder scoped to that display, from step 1 to step 7;
  - a Ready Hetzner `cx23` cloud whose image carries the bridge helper.
- On this computer, a throwaway dev server on loopback only, for example
  `python3 -m http.server 8765 --bind 127.0.0.1` in an empty directory.
- A device on the network that serves HTTP, called `printer.local` at
  `192.168.1.50` below. Use its real address while testing, but only these synthetic
  names in public reports.

Drive the worker with `horizon-cloud-worker local-network` over the cloud's SSH
connection, so no model key is placed on the worker.

## Steps

1. Switch on **Share local network**. Expect **Scope: the whole network**.
   `local-network forward 192.168.1.50 80` and a `curl` through the returned port
   succeed. `local-network forward localhost 8765` is refused with "Outside the
   bridged local network: … the Horizon computer only as localhost on ports the
   owner opened".
2. Hold a connection to `192.168.1.50:80` open (for example with the driver from
   the open connections smoke). Open **Scope**, enter `192.168.1.50:631` under
   Devices, and press **Apply scope**. Expect **Scope: 1 device**, and the held
   connection's row disappears within a second.
3. `forward 192.168.1.50 631` succeeds. `forward 192.168.1.50 80` and a forward to
   any other device are refused. `local-network discover` lists only
   `192.168.1.50`, and `local-network probe 192.168.1.50 80` is refused, naming
   port 80.
4. Enter `8765` under This computer's own ports and press **Apply scope**. Expect
   **Scope: 1 device · 1 port on this computer**. `forward localhost 8765` and a
   `curl` through it return the dev server's listing. `forward localhost 22` is
   still refused.
5. Enter `10.0.0.5` under Devices and press **Apply scope**. Expect "10.0.0.5 is
   not a device on the bridged network" under the button, and the header still
   reads **Scope: 1 device · 1 port on this computer**.
6. Clear both fields and press **Apply scope**. Expect **Scope: the whole
   network**. `forward 192.168.1.50 80` succeeds again, and `forward localhost
   8765` is refused.
7. Switch sharing off and on. Expect **Scope: the whole network**.

## Record

For each step, record pass or fail with a screenshot of the card and the worker
command output. Decode frames from the recording at steps 2 and 4 to confirm the
header and rows change. Record the candidate commit and binary hash with the
evidence. Keep screenshots, recordings and command output private; public reports
use only synthetic names and addresses.
