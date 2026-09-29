# Local Network Bridge Network Change Smoke Plan

Use this checklist to check that sharing stops when this computer moves to another
network, asks again, and resumes by itself only on the same network (#987 M2).

## Goal

- Sharing a network, then moving this computer to another one, stops sharing within
  a few seconds. The card names the new network and offers to share it; nothing
  resumes by itself, and the scope is forgotten.
- Sharing the new network from the card starts a fresh bridge on it.
- A reconnect on the same network pauses sharing and resumes it by itself.

## Setup

Moving a real computer between networks during a test is disruptive, and a GIF or
screenshot must not show real network details. So on Linux, run the candidate on
two synthetic networks inside an unprivileged network namespace:

- `bwrap --dev-bind / / --ro-bind <resolv.conf> /etc/resolv.conf --unshare-user
  --unshare-net --uid 0 --gid 0 <candidate> --new-session` with a private `HOME`,
  `XDG_RUNTIME_DIR` and `DISPLAY`. The `resolv.conf` names `10.0.2.3` and
  `10.0.3.3`, slirp4netns's DNS on the two networks. Ubuntu restricts unprivileged
  user namespaces for other tools; `bwrap` is allowed.
- Two `slirp4netns --mtu=65520 --disable-host-loopback` instances attached to the
  candidate's PID: one with `--configure` on `tap0` (network A, `10.0.2.0/24`), one
  with `--cidr=10.0.3.0/24` on `tap1` (network B, not yet configured).
  `--disable-host-loopback` keeps this computer's own loopback services
  unreachable through the synthetic gateway. `slirp4netns` and `libslirp0` can be
  unpacked from their packages into a scratch directory with `apt-get download`
  and `dpkg-deb -x`.
- The same isolated desktop, native VNC Device panel and recording as the other
  bridge smoke plans, and a Ready Hetzner `cx23` cloud with the bridge helper.

To move to network B, run inside the namespace
(`nsenter --preserve-credentials -U -n -t <candidate PID>`):
`ip link set tap1 up; ip addr add 10.0.3.100/24 dev tap1; ip route replace default
via 10.0.3.2 dev tap1`.

## Steps

1. Switch on **Share local network**. Expect **Sharing 10.0.2.0/24**. Narrow the
   scope to `10.0.2.3` and expect **Scope: 1 device**.
2. Move to network B. Within a few seconds, expect **Sharing stopped: this computer
   moved to another network (10.0.3.0/24). Nothing is shared until you share it.**
   and a **Share 10.0.3.0/24** button. Nothing resumes by itself.
3. Press **Share 10.0.3.0/24**. Expect **Sharing 10.0.3.0/24** and **Scope: the
   whole network**. The worker's `local-network status` reports that subnet.
4. Press **Reconnect cloud**. Expect **Sharing paused: the cloud disconnected or this
   computer slept** while it reconnects, then **Sharing 10.0.3.0/24** again with a
   new worker-side proxy port.

Sleep cannot be triggered without suspending the test computer. Unit tests cover it:
a wall clock running ahead of the monotonic clock pauses sharing, which resumes only
on the same network.

## Record

For each step, record pass or fail with a screenshot of the card and the worker's
status. Record the candidate commit and binary hash. The synthetic networks keep
screenshots, the recording and the PR GIF free of real network details.
