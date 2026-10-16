# Worker TCP exposure gate

This default-off component is an intermediate part of issue #1335.
It does not complete M2 or add a UI, CLI, MCP, provider or worker-control operation.
The Worker `exposure-gate` feature consumes the separate shared policy prerequisite and compiles the worker engine.
No default path calls the engine.

## Trust boundary

Policy JSON and incoming TCP bytes are untrusted.
`ExposureRequest::from_json` bounds encoded input before decoding.
Validation rejects unknown fields, invalid bounds, protected targets and empty restrictions.
Only explicit Public mode accepts any source.
Missing mode is Off, and Off creates no listener.

The gate gets a CIDR source from `peer_addr` on the original accepted stream.
It never gets a source from a request, PROXY header, DNS name or forwarded address field.
IPv4-mapped IPv6 addresses use the same IPv4 accounting bucket.
An IP allow-list is not authentication.
A proxy or provider can change the TCP peer; this component does not recover the original sender.
Provider source preservation needs separate real qualification.

Shared policy syntax may represent DNS names, tags and TailnetOnly, but this worker returns an explicit unsupported error before binding, including mixed CIDR/name/tag lists.
The gate does not call WhoIs or construct an authenticated tailnet principal.
A loopback four-tuple or a well-formed PROXY header cannot grant named-source authority.
A later producer must bind the accepted stream to the original trusted daemon's connection and identity.
That producer must pass real qualification before these modes or the feature are activated.

## Listener and target ownership

The production constructor requires the trusted worker root controller and an active Tokio runtime with I/O and timers.
Root process custody does not replace an owner decision or current workspace membership.
Those checks belong to the future controller, outside this component.
No serialized record grants permission to call the constructor.

One gate exclusively binds one reserved ingress port, 20000–20031, on IPv4 and IPv6 wildcard listeners.
IPv6 is separate from IPv4; either bind failure closes the partial reservation.
There is no existing-descriptor adoption, address fallback, reuse-port option or caller-supplied listener.
The target is always the literal worker loopback `127.0.0.1` and an explicit validated service port.
Ports 22, 5900, 9222 and 2375 are refused.
Worker control/proxy ports 1055, 1056, 47280 and 47281 and the reserved ingress/backend pools 20000–20063 are refused too.
The component adds no TLS or authentication to raw TCP.

Exclusive binding lasts while the gate owns its listener descriptors.
After release, this component does not prevent another process in the same network namespace from binding the unprivileged ingress port.
Provider/public activation requires separate proof of root-only ingress and backend custody, including keeper death and uncertain mapping cleanup.
This component does not supply that integration guarantee.

## Bounds and closure

- At most 128 admitted connections per gate, with a validated per-source limit.
- At most 256 retained source-accounting buckets and 32 CIDR rules.
- At most 16 KiB of encoded policy input.
- Fixed 8 KiB user buffers per direction, with no payload queue.
- Socket buffers request 16 KiB; effective send and receive sizes must each stay within 64 KiB.
- Ordered connection, source and gate byte budgets; the gate budget cannot exceed 64 GiB.
- Connect timeout at most 3 seconds, idle timeout at most 60 seconds and flow lifetime at most 15 minutes.
- An immutable monotonic gate lifetime at most eight hours, with no renewal or extension API.

Pending backend dials count as admitted connections.
Byte accounting charges before writes; failed writes keep their conservative charge.
Budget exhaustion refuses new reservations and lets already reserved bytes drain before shutting the affected flows.
Outstanding reservations cover both directions and concurrent flows, so a partial write does not discard another accepted reservation.
Explicit cancellation, an expired deadline or an I/O failure still shuts the flow immediately, without refunding its charge.
Source charges survive connection closure, so reconnecting does not reset a source budget.
The source-history cap refuses new peers without evicting earlier accounting.

Cancel stops admission and shuts tracked transports before any task join.
Explicit close then cancels and drains every flow task and retires the listener descriptors before it acknowledges success.
Task or listener failure produces an error, not a clean-close acknowledgement.
Drop initiates fail-closed cancellation; callers need explicit close for an awaited acknowledgement.
TCP TIME_WAIT can prevent immediate address reuse after all descriptors close.

The gate has no persistence, crash recovery, provider mapping custody or release protocol.
Its monotonic cap is not the complete per-service eight-hour lifecycle or renewal feature.
Restart, owner control, original mapping reconciliation, interface parity and durable expiry remain later outcomes.
Do not release a provider mapping or approve a new attempt from these counters or policy facts.

## Modules and qualification

`horizon-cloud-protocol::exposure` owns bounded configuration and pure CIDR matching.
`horizon-cloud-worker::exposure_gate` owns exclusive listeners, immutable lifetime and explicit close.
Its `state` leaf owns admission and accounting; `relay` owns cancellable literal-loopback dials and bounded byte transfer.
Colocated tests use real accepted sockets and private loopback fixtures.
They do not prove a provider, Tailscale source attribution or owner/public parity.

Use the [permanent procedure](../testing/procedures/worker-exposure-gate.md) for local and approved worker-component checks.
