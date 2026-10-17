# Original TCP producer

The Unix backend below replaces the earlier TCP backend. Independent review found that the TCP backend admitted a generic same-daemon proxy connection carrying an attacker-supplied header. Earlier positive results do not establish the corrected boundary. The [qualification report](../testing/reports/2026-10-16-trusted-tcp-producer.md) records the current results.

The Linux Worker `trusted-tcp-producer` feature stays off by default.
It identifies a tailnet TCP connection through the original daemon's private Unix Serve backend.
It adds no Gate admission, Serve mapping, keeper, provider effect or public operation.

## Input and authority boundary

A caller selects only the fixed Worker or Kernel runtime and reserved virtual/backend ports.
The Worker runtime uses `/run/horizon-tailnet/tailscaled.sock`.
The Kernel runtime uses `/run/tailscale/tailscaled.sock`.
The component accepts no arbitrary runtime path or serialized identity as authority.

`Producer::pin` requires root and retains the original UNIX connection and peer credentials.
It retains the original pidfd, executable descriptors, process start and namespaces.
It checks root-owned paths and files and the actual procfs before it accepts stream input.
A stopped or replaced daemon cannot become the original daemon.

The component owns a Unix listener under a fixed root-owned directory with mode 0700.
Worker backends use `/run/horizon-tailnet/producer/<backend>.sock`;
Kernel backends use `/run/horizon-kernel-producer/<backend>.sock`.
The socket has mode 0600, and the reserved backend number identifies its filename; no TCP backend port is opened.
An existing socket is never removed or adopted to start another producer.
The original path inode, trusted ancestors and permissions must remain unchanged.
The accepted stream must name that backend locally, and its kernel peer credentials must identify the same original root daemon.
These checks repeat around header parsing and identity queries.

The pinned Tailscale runtime patch requires explicit `--horizon-flow-cancel` for this Unix Serve PROXY-v1 contract, using the virtual service port as the destination port. Ordinary Unix PROXY1 without the opt-in cannot pass the updated receiver.
Its generic SOCKS and HTTP proxies remain TCP dialers and cannot connect to this Unix endpoint.
The private directory also excludes direct connections from worker agents.
Unmodified Tailscale v1.102.5 rejects Unix plus PROXY, so it cannot qualify a positive connection here.
The patch retains the existing tailnet, enrollment and outbound proxy capability; it adds no network stack or relay.

A bounded PROXY v1 header supplies comparison fields only after those kernel checks.
The exact source then goes to the pinned daemon's bounded WhoIs command.
Running Self identity and enrollment generation must remain the same throughout identification.
The parsed StableID, complete DNS name and tags supply no authority without the original stream proof.

`AcceptedPeer` retains that same stream and its compared identity.
It has no public constructor, Clone or Deserialize implementation.
`into_stream` consumes the peer and transfers an opaque `AcceptedStream`, never a bare Unix stream or replacement input.
The stream delegates payload I/O and AsFd only to the original payload; it has no public constructor, Clone or Deserialize.
A separate original per-flow cancellation endpoint remains private.

The receiver reads exactly the first header byte `P` with bounded `recvmsg` and exactly one SCM_RIGHTS descriptor.
It uses CLOEXEC receipt and requires nonblocking connected unnamed AF_UNIX SOCK_STREAM endpoints, no listener, and the same original root daemon peer credentials.
It rechecks original process, pidfd and payload/listener custody before and after receipt and subsequent observations.
Extra, truncated or late header control messages, wrong prefixes and missing descriptors refuse; all delivered descriptors have RAII owners.
The remaining header is read one byte at a time, never consuming application bytes.
Control data and parsed headers are comparison/protocol data, not an independent source of identity.

## Bounds and cancellation

At most sixteen identifications can remain pending.
Each identification uses one three-second deadline across its observations and commands.
The PROXY header has a 108-byte limit.
CLI output and procfs input have explicit byte limits.

Cancellation, an unpolled future and capacity refusal shut the original stream.
After descriptor receipt, every failure also shuts and drops the original cancellation endpoint.
Whole-stream cancel takes that endpoint and calls SHUT_RDWR immediately, including while a private finish reader or caller Arcs remain retained.
Normal payload write-half EOF keeps cancellation alive; last stream Drop cancels both.

The daemon sends exactly DRAINED byte `0x01`, then EOF on the control endpoint only after both payload copies and half-closes succeed.
The public original-bound `finish` method uses a private endpoint reader and requires that exact acknowledgement; EOF alone, extra/wrong bytes or errors fail closed.
It preserves an interrupted acknowledgement stage on the same original endpoint and never duplicates its FD.
A returned finish error cancels both original channels immediately, even if the caller retains the stream or ignores that error.
The consuming caller must bound finish by its captured flow/lifetime deadline and cancellation before treating the original flow as complete. This Producer-only component adds no Gate transport or lifecycle integration.
No new timeout, idle poll, authority factory or detached lifetime watcher is added.
The acknowledgement proves daemon forwarding was queued and half-closed, not delivery to the remote application.
The original child supervisor must kill and wait for its child before it returns its permit.
An unconfirmed wait retains uncertainty in that child pool.
Consuming retirement waits for the original identification callers and child supervisors, closes the listener and removes only its original socket path.
A replacement path is preserved, and uncertainty cannot produce a clean retirement result.

## Limits and later composition

This component does not create a daemon, enroll a device or change Serve/Funnel state.
It does not establish workspace membership or native owner approval.
It does not consume a protected host receipt or prove publication and source preservation.
Its reserved logical backend is component equipment, not a protected production keeper route.
The existing CIDR/Public Gate still refuses named-source and TailnetOnly policies.

Actual Worker and Kernel daemon checks belong to the component qualification.
They do not replace full M2 protected runtime, owner, Native, provider or public-interface qualification.
M1 one-device sharing keeps its no-expiry contract.
Read the [procedure](../testing/procedures/trusted-tcp-producer.md) before a real-daemon test.

## Modules

`mod.rs` owns the producer and opaque peer.
`ancillary.rs` owns bounded one-byte descriptor/header receipt and rejection cleanup.
`accepted.rs` owns opaque payload/control lifetime, cancellation and drain acknowledgement.
`daemon.rs` owns original process, UNIX socket and executable custody.
`backend.rs` owns the private Unix listener and its original path.
`procfs.rs` bounds reads of original process observations.
`proxy.rs` reads the bounded untrusted header.
`whois.rs` parses bounded daemon facts and supervises bounded CLI children.
`tests.rs` contains the component regressions and the ignored real-daemon selector.
Its test-only TCP observer proves that the spoofing probes reach the same daemon's actual generic proxies; TCP ownership is not admission authority.
