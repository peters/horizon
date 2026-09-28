# Local Network Bridge Open Connections Smoke Plan

Use this checklist to check the **Open connections** list on a cloud card on a real
cloud (#987 M2).

## Goal

With **Share local network** on:

- the card lists each relayed connection with its destination, the data it relayed
  and how long it has been open;
- a connection appears when a worker process opens it and disappears when it ends;
- opening, closing and scrolling the list never moves the controls below it while
  connections start and end.

## Setup

- A Linux or macOS computer on a home or office network with at least one device
  that serves TCP, for example a printer's or router's web page.
- A candidate build of the PR head, launched isolated from your own sessions:
  `target/debug/horizon --new-session`. On a headless Linux box, use a private
  Xvfb display with a window manager, as in the other UI smoke plans.
- A Ready cloud. The cheapest is a Hetzner `cx23` image-only profile whose image
  carries this revision's `horizon-cloud-worker` and worker scripts. Delete it
  when you finish.

No agent needs to run on the worker. Drive the worker's `horizon-local-network`
MCP server or CLI over the cloud's SSH connection, so no model key is placed on it.

## Steps

1. On the Ready card, switch on **Share local network**. Expect
   **Sharing <subnet> · 0 open · 0 B** and a collapsed **Open connections (0)**
   header.
2. Open the header. Expect **No connections open.** in a list with room for six
   rows. Note the position of the controls below the list.
3. On the worker, call `local_network_forward` for the LAN device's web port and
   fetch a page through the returned loopback port, keeping the connection open
   (for example `curl --limit-rate 2k` of a larger resource, or `nc` held open).
   Expect a row such as `192.168.1.216:80 · 1.2 KB · 4 s` within a second, with
   growing bytes and age, and the header count at 1.
4. Open seven or more connections at once. Expect newest first, a scroll bar in the
   list, and the controls below the list at the same position as in step 2.
5. End the connections. Expect their rows to disappear within a second and the
   controls below to stay put.
6. Collapse the header, then switch sharing off. Expect the list to go away with
   the rest of the sharing status.

## Record

For each step, record pass or fail with a screenshot of the card. For step 4,
also record the y position of the control below the list before and after.
