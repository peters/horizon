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
- **Reconnecting: …** with the reason, when the SSH session to the worker
  dropped. Horizon retries on its own while the switch stays on.
- A message instead of the switch staying on when there is nothing to share, for
  example when this computer is not on an IPv4 network, the network is wider than
  `/16`, or the default route is a VPN's point-to-point link.

Switch it off to stop the bridge at once. When the cloud stops being connected
and Ready (it disconnects, stops or rebuilds), the bridge stops too and the card
shows **Sharing paused: cloud disconnected** with the switch still on. Once you
reconnect or resume that cloud and it is Ready again, sharing restarts by itself
with a new session; agents check the status and forward again. Switch it off while
paused to stop waiting. A paused switch never outlives Horizon: after a restart it
is off.

The worker image must include the bridge helper. An image built before this
feature makes the card say so; rebuild the cloud's image.

## What is shared

The scope is the IPv4 subnet of the network that carries this computer's default
route when you switch the bridge on, for example `192.168.1.0/24`. Horizon checks
every connection on this computer, never on the worker:

- Only host addresses inside that subnet are reachable, and only when this
  computer would send the connection from its own address on that network.
- This computer itself is never reachable, through any of its addresses,
  including `localhost`. Loopback, link-local, multicast and broadcast addresses
  are refused, and so is IPv6, except an IPv4-mapped address such as
  `::ffff:192.168.1.50`, which is judged as the IPv4 address it carries.
- Names such as `printer.local` are resolved by this computer's resolver, and the
  connection goes to exactly the address that was checked.
- When this computer moves to another network, every connection is refused until
  you switch the bridge off and on again.

Some changes are not detected yet. A different network that hands this computer
the same address on the same interface and subnet (two Wi-Fi networks that both
use `192.168.1.0/24`, say) looks unchanged, and the route check compares source
addresses, so a route that you set up yourself to send this network's traffic out
of another interface with the same source address is not caught. Switch the bridge
off when you change networks.

## Limits

- **TCP only.** UDP does not cross the bridge: no mDNS or SSDP from the worker, no
  RTSP over UDP, no ping. Use RTSP over TCP (for example `ffmpeg -rtsp_transport tcp`).
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
operations as `horizon-cloud-worker local-network status|forward|unforward`:

- `local_network_status`: whether the bridge is on, the bridged subnet, the SOCKS5
  proxy address on the worker's `127.0.0.1`, and the pinned forwards. When the
  bridge is off it says that only you can turn it on.
- `local_network_forward` with a host and port: pins that device to a port on the
  worker's `127.0.0.1`, so any TCP tool works unchanged, for example
  `ffmpeg -rtsp_transport tcp -i rtsp://127.0.0.1:<port>/stream`. Horizon checks
  the destination before the port opens.
- `local_network_unforward` with that worker port: closes the forward and its
  connections.

Tools and browsers that accept a SOCKS5 proxy can use the proxy address directly,
for example `curl --socks5-hostname 127.0.0.1:<port> http://192.168.1.1/`.

Refusals read, for example, "Outside the bridged local network: only devices on
the shared subnet are reachable, never the Horizon computer itself", "Device not
reachable from the Horizon computer" or "The device refused the connection on
that port". Forwards end when the bridge stops or reconnects; agents check the
status and forward again.

## How it works

Switching the bridge on starts a SOCKS5 proxy on this computer's loopback and one
SSH process to the worker, with the cloud's own pinned host key and no agent or X11
forwarding. That SSH process forwards a private socket on the worker to the proxy
and runs a small helper that serves the proxy address, the forwards and the status
on the worker's `127.0.0.1`. The helper stops, and removes its sockets and forwards,
when the SSH session ends or stops sending its heartbeat for a minute.
