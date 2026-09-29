# Local Network Bridge

Local Network Bridge lets a cloud's agents reach devices on the network this
computer is on: a camera, a dev board, a printer or a router's web page. Horizon
relays the traffic through its own SSH connection to the worker, opened with the
cloud's pinned host key, so no VPN, router change or extra hardware is needed. It is part of
[Cloud workspaces](cloud-workspaces.md).

## Turning it on

On a Ready cloud's card, switch on **Share local network**. It is off by default
for every cloud. Nothing turns it on for you: it is never read from repository
YAML, saved state or an agent request, and it is off again after Horizon
restarts.

While it is on, the card shows what the bridge is doing:

- **Connecting to share 192.168.1.0/24…** while the worker end starts.
- **Sharing 192.168.1.0/24 · 2 open · 12.4 MB**: the bridged subnet, the
  connections open now and the data relayed since it started.
- **Open connections (2)**, collapsed until you open it: each relayed connection,
  newest first, with where it goes, the data it relayed and how long it has been
  open, for example `printer.local:80 (192.168.1.50) · 12.4 KB · 3 min`. A name
  the agent asked for is shown with the address it reached. Pinned forwards
  appear here too, because they travel through the same proxy. The list keeps
  one height and scrolls, so connections starting and ending do not move the
  controls below it.
- **Reconnecting: …** with the reason, when the SSH session to the worker
  dropped. Horizon retries on its own while the switch stays on.
- A message instead of the switch staying on when there is nothing to share, for
  example when this computer is not on an IPv4 network, the network is wider than
  `/16`, or the default route is a VPN's point-to-point link.

Switch it off to stop the bridge at once. When the cloud stops being connected
and Ready (it disconnects, stops or rebuilds), or this computer sleeps, the bridge
stops too and the card shows **Sharing paused: the cloud disconnected or this
computer slept** with the switch still on. Once the cloud is Ready again, sharing
restarts by itself with a new session, keeping the scope you set, but only if this
computer is still on the same network; agents check the status and forward again.
Switch it off while paused to stop waiting. A paused switch never outlives
Horizon: after a restart it is off.

When this computer moves to another network, for example a new Wi-Fi, sharing
stops within a few seconds, whether it was running or paused; a paused bridge stops
waiting for the cloud then. The card says
**Sharing stopped: this computer moved to another network (10.0.0.0/24)**, and
nothing is shared until you press **Share 10.0.0.0/24**, which starts again on the
new network with the whole network in scope, or switch sharing off. The offer
follows this computer if it moves again, and appears only while the cloud is
connected and Ready; during a reconnect it waits until that reconnect is Ready. A
paused bridge that finds no network it can share when it would resume stops the
same way. The scope you set belonged to the old network and is
forgotten.

On Windows, sleep is not noticed yet, because the clock Horizon compares with keeps
running through suspend there; a move to another network after waking still is.

The worker image must include the bridge helper. An image built before this
feature makes the card say so; rebuild the cloud's image.

## What is shared

The scope is the IPv4 subnet of the network that carries this computer's default
route when you switch the bridge on, for example `192.168.1.0/24`. Horizon checks
every connection on this computer, never on the worker:

- Only host addresses inside that subnet are reachable, and only when this
  computer would send the connection from its own address on that network.
- This computer itself is not reachable through any of its addresses, including
  `localhost`, unless you open one of its ports in the scope (see below). An opened
  port is reachable as `localhost`, `127.0.0.1` or `::1`, and only those.
- Other loopback, link-local, multicast and broadcast addresses are refused, and so
  is IPv6, except `::1` for an opened port and an IPv4-mapped address such as
  `::ffff:192.168.1.50`, which is judged as the IPv4 address it carries.
- Names such as `printer.local` are resolved by this computer's resolver, and the
  connection goes to exactly the address that was checked.
- When this computer moves to another network, every connection is refused, and the
  card stops sharing and asks you again (see above).

Some changes are not detected yet. A different network that hands this computer
the same address on the same interface and subnet (two Wi-Fi networks that both
use `192.168.1.0/24`, say) looks unchanged, and the route check compares source
addresses, so a route that you set up yourself to send this network's traffic out
of another interface with the same source address is not caught. Switch the bridge
off when you change networks.

## Narrowing the scope

While sharing is on, the card's **Scope** section says what the bridge reaches,
for example **Scope: the whole network** or **Scope: 1 device · 1 port on this
computer**. Open it to change that:

- **Devices**, one per line: an address on the bridged network, and optionally its
  ports after a colon, for example `192.168.1.50` or `192.168.1.60:22,80`. Leave it
  empty for every device on the network. With a list, only those devices are
  reachable, each on its listed ports or on any port when none are listed.
- **This computer's own ports**, for example `3000, 8080`, to let the worker reach
  services on this computer's loopback, such as a dev server, as `localhost:3000`.
  Leave it empty to keep this computer closed. Only the ports you list open; this
  computer's other ports and its addresses on the network stay refused.

Press **Apply scope**. The new scope applies at once. Connections it no longer
allows close, discovery lists only the devices it reaches, and a probe skips ports
outside it. An address that is not a device on the bridged network is refused, and
the previous scope stays.

The scope lives only in memory, like the switch, and only you set it. Nothing
reads it from repository YAML or from an agent. It is kept while sharing is
paused, so a resumed bridge starts no wider than you left it. If it no longer fits
the network when sharing resumes, for example after a move to another Wi-Fi,
sharing stops and says why instead of widening. Switching sharing off forgets it.

## Limits

- **TCP only.** UDP does not cross the bridge: no mDNS or SSDP from the worker, no
  RTSP over UDP, no ping. Use RTSP over TCP (for example `ffmpeg -rtsp_transport tcp`).
  To find devices, agents ask this computer instead; see
  [Finding devices](#finding-devices).
- **Bandwidth.** Traffic crosses this computer's uplink twice, so a video stream is
  limited by its upload speed.
- **Bounds.** At most 64 connections at once and 64 GiB relayed per bridge; switch
  it off and on to start counting again.
- **Everything on the worker can use it.** While the bridge is on, every process on
  a dedicated worker can use its proxy and forwards, and so can web pages open in
  the worker's browsers, which can reach a forward's `127.0.0.1` port. Shared
  workers are not supported.
- **One Horizon at a time.** While one computer shares its network with a worker,
  another computer's bridge to the same worker waits and reports that the worker
  is already bridged.

## What the agent sees

Agents on the worker get a `horizon-local-network` MCP server, and the same
operations as `horizon-cloud-worker local-network status|discover|probe|forward|unforward`:

- `local_network_status`: whether the bridge is on, the bridged subnet, the SOCKS5
  proxy address on the worker's `127.0.0.1`, the pinned forwards, and whether
  discovery works and with what on this computer. When the bridge is off it says
  that only you can turn it on.
- `local_network_discover`: the devices on your network, found by this computer.
  See [Finding devices](#finding-devices).
- `local_network_probe` with a host and optionally up to 16 ports: which of those
  ports the device accepts connections on. See [Checking a device's ports](#checking-a-devices-ports).
- `local_network_forward` with a host and port: pins that device to a port on the
  worker's `127.0.0.1`, so any TCP tool works unchanged, for example
  `ffmpeg -rtsp_transport tcp -i rtsp://127.0.0.1:<port>/stream`. Horizon checks
  the destination before the port opens.
- `local_network_unforward` with that worker port: closes the forward and its
  connections.

Tools and browsers that accept a SOCKS5 proxy can use the proxy address directly,
for example `curl --socks5-hostname 127.0.0.1:<port> http://192.168.1.1/`.

Refusals read, for example, "Outside the bridged local network: only the devices
the owner shares are reachable, on the ports shared for each, and the Horizon
computer only on ports the owner opened, as localhost, 127.0.0.1 or ::1", "Device not
reachable from the Horizon computer" or "The device refused the connection on
that port". Forwards end when the bridge stops or reconnects; agents check the
status and forward again.

## Finding devices

An agent can ask what is on your network instead of being told addresses, for a
demo such as "find the printer and show me its status page". The browse runs on
this computer, never on the worker, and only when an agent asks:

- **mDNS / Bonjour**: devices that announce services, such as printers, TVs,
  speakers, cameras, dev boards and other computers, with the names they give
  themselves, their services, ports and details such as the printer model.
- **SSDP / UPnP**: routers, TVs and media devices, with their device type, server
  string and the address of their description page, which the agent can fetch
  through the bridge.
- **The neighbor table**: devices this computer has recently exchanged traffic
  with. Only their addresses are shared, never their hardware addresses.

It looks for about three seconds and repeats the same answer to requests in the
next 15 seconds, so asking again soon sends nothing on the network. Only one browse
runs at a time. The answer lists at most 256 devices, each with at most 4 names and
16 services, and shortens the text that devices choose.

Every device passes the same scope as a connection: only addresses on the bridged
subnet are listed, never this computer, and nothing is sent when this computer has
left the bridged network. Queries go out through the bridged network's interface
only. Horizon does not answer mDNS for this computer, and it never sweeps the
subnet address by address.

What each system provides:

| This computer | mDNS | SSDP | Neighbor table |
|---|---|---|---|
| Linux | yes | yes | yes |
| macOS | yes | yes | yes, from `arp` |
| Windows | yes | yes | yes, from `arp` |

Discovery from Windows is not fully tested yet, and Windows Firewall can hide
devices that answer mDNS or SSDP; `local_network_status` tells agents so. A source
that fails adds a note to the answer instead of failing the whole request.

A worker helper whose owner's Horizon is older than discovery reports that
discovery is unavailable; update Horizon on this computer.

## Checking a device's ports

Devices do not always announce what they serve. An agent can ask which of a few TCP
ports one device accepts connections on, for example whether a camera serves RTSP
before it forwards port 554:

- One device per request, named by its address or host name. Horizon resolves the
  name on this computer and probes only a device the bridge could reach anyway:
  never this computer, never outside the bridged subnet.
- At most 16 ports. Without a list, Horizon tries 22, 80, 443, 554, 631, 1883, 3000,
  5000, 8000, 8080, 8123, 8443, 8554 and 9100.
- A plain TCP connect per port, closed at once; nothing else is sent. Four ports at
  a time, 1.5 seconds each.
- At most 6 probes a minute per bridge, one at a time: a probe asked for while
  another runs is refused at once, and the agent tries again a few seconds later.
  Probes refused before they connect, for example for a host outside the scope,
  do not count.

The answer lists open, refused and silent ports, and later discovery answers include
the open ports. Switching the bridge off stops a probe or a browse at once: no new
connection attempt starts, and one already under way may finish within its 1.5
seconds, when its connection is closed without use. Horizon never walks the subnet on its own.

## How it works

Switching the bridge on starts a SOCKS5 proxy on this computer's loopback and one
SSH process to the worker, with the cloud's own pinned host key and no agent or X11
forwarding. That SSH process forwards a private socket on the worker to the proxy
and runs a small helper that serves the proxy address, the forwards and the status
on the worker's `127.0.0.1`. The helper passes discovery and probe requests to this
computer over the same SSH session, and this computer checks each one before it
answers. The helper stops, and removes its sockets and forwards,
when the SSH session ends or stops sending its heartbeat for a minute.
