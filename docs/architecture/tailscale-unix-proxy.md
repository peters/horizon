# Tailscale Serve Unix PROXY boundary

The worker recipes build pinned Tailscale v1.102.5 with Unix Serve PROXY v1 and
an explicit Linux flow-cancellation capability. This transport grants no source
identity, enables no Horizon exposure feature and changes no published image pin.
The runtime capability and the later Producer/Gate consumer have separate gates.

## Explicit opt-in

`serve --tcp=<virtual-port> --proxy-protocol=1 unix:/<socket>` writes a v1 header
before application bytes, using the source-family node address and original
virtual ingress port. Unix v2, zero ingress and missing node-family address refuse.
`--horizon-flow-cancel` additionally selects the descriptor contract below. It
is a Serve-only boolean, default false, represented by
`TCPPortHandler.HorizonFlowCancel` (JSON `HorizonFlowCancel`, omitted when false).
It requires Linux raw TCP, an absolute filesystem Unix target and PROXY1, with
no HTTP/HTTPS, TLS termination or Funnel. Invalid explicit opt-in refuses.

CLI validation precedes config mutation. Direct `SetServeConfig` validation
covers top-level TCP handlers, recursive Foreground configs and Services TCP
handlers before persistence; the actual handler also checks restored entries.
An opted-in raw transport must refuse Funnel on its matching ingress port across
config scopes, direct API updates and actual ingress, not only the CLI flag path.
Clone/view and JSON/status round trips preserve the field. False/absent keeps
existing TCP v1/v2, Unix without PROXY, ordinary Unix PROXY1, generic HTTP/SOCKS
proxies, enrollment and networking behavior. No protocol is silently upgraded.

## One original payload and one control endpoint

On an opted-in flow, the daemon sends exactly the first header byte `P` with
exactly one SCM_RIGHTS descriptor, then sends the remaining header before payload.
The descriptor is one endpoint of a private, unnamed AF_UNIX SOCK_STREAM
socketpair with nonblocking/CLOEXEC state. There is no extra payload framing.
The sending goroutine owns its local sent-end handle through `WriteMsgUnix`;
it closes that copy immediately afterward, never shutdowns it or retries an
ambiguous transfer. Cancellation interrupts the original backend connection,
not a concurrently reused numeric descriptor. Temporary duplicates are closed.

Payload read EOF half-closes only the other payload write side. The reverse copy
and control custody remain live. Frontend CloseWrite must reach the real TCP or
userspace gonet connection; a wrapper's silent success is insufficient.
The consumer sends no control bytes and never half-closes control. Consumer EOF,
unexpected data/read error, daemon shutdown or header/copy/CloseWrite error
cancels the whole flow, including blocked header and response writes.

Only after both copies and both required CloseWrite operations succeed does the
daemon send exactly `DRAINED = 0x01` on control, close that endpoint and join its
control reader. The consumer requires exactly `0x01` followed by EOF. EOF alone,
wrong/extra data, missing EOF, death or custody loss cannot complete a flow.
Cancellation observed before success commits wins; the consumer's own deadline,
phase and cancellation still override an acknowledgement race. Failed/ambiguous
acknowledgement is failure without retry. Every started sender, copy and control
reader is joined on every return, retaining the first meaningful failure.

DRAINED proves daemon copy completion and frontend half-close, including bytes
queued to that connection. It does not prove remote application delivery.
External byte/EOF observations remain necessary. Whole-flow consumer cancellation
must shutdown both original payload directions and the original control endpoint
before dropping custody; a retained private reference must not postpone it.
Ordinary payload write EOF must preserve control until acknowledgement or failure.

## Source and lifetime authority remain separate

Application bytes and PROXY-looking text are untrusted. A same-daemon loopback
TCP stream cannot distinguish Serve from generic HTTP/SOCKS proxy output.
The consumer must retain a root-owned 0700 directory, original Unix listener
and the pinned original root daemon's PID/start/executable/namespace custody.
The root controller exclusively owns the Serve route to that listener with
PROXY1 and the explicit descriptor contract; unrelated privileged routes to the
same socket are not trusted ingress. A privileged local attacker remains outside
this isolation. Use the exact qualified CLI/daemon pair, not an inferred minimum
version. Old/ordinary Unix streams lacking the descriptor must refuse, not fall back.

The subsequent consumer must own every received descriptor immediately; reject
missing, extra, truncated, late or wrong control messages and close every delivered
FD on refusal. It verifies the same original daemon before/after receipt and a
connected, non-listening, unnamed AF_UNIX stream with matching kernel peer
credentials. It seeds its bounded v1 parser with the consumed `P`, without reading
application bytes. Generation, destination and current same-daemon identity checks
still govern admission. A path, header, config bool, version, descriptor, saved
metadata or acknowledgement alone grants no identity.

The later opaque consumer must retain cancellation custody through exact ACK+EOF
under its existing flow/grace deadlines, with no new grace or read allowance.
Genuine original Kernel and userspace Worker first/final cases, generic-proxy
forgery refusal and full original-stream/listener retirement remain separate
consumer acceptance gates. This runtime does not activate Producer, Gate, M2,
Core/UI/provider routes, public MCP/CLI controls or Native behavior.

## Qualification boundary

The shared pinned builder runs complete upstream `./ipn`, `./ipn/ipnlocal` and
`./cmd/tailscale/cli` tests on native Linux amd64 and arm64 before building the
pair and retaining notices. Reproducibility requires independent equal-input
builds, not matching markers alone. New descriptor/half-close/ACK behavior needs
current-head evidence: earlier non-descriptor green images or tests cannot
qualify it. See [the procedure](../testing/procedures/tailscale-unix-proxy.md).
