# Original producer to Restricted TCP Gate

This Linux-only `trusted-exposure-gate` Worker feature stays off by default.
It composes the existing Gate and trusted TCP Producer for Restricted CIDR,
canonical full DNS-name and tag rules. It is an M2 prerequisite, not public M2
activation or completed owner/source qualification. No ordinary command calls it.

## Authority and untrusted inputs

Policy JSON, accepted stream bytes and PROXY headers are untrusted. Syntax and
copied identity facts grant no admission. `Gate::bind_with_producer` consumes one
original root-pinned Producer, not a path, peer DTO, callback or identity clone.
The corrected Producer retains original daemon credentials, process/executable/
start/namespaces, a root-private Unix listener and its original path inode.
The accepted stream's kernel credentials must identify that same daemon around
the bounded header, Running Self generation and fresh WhoIs observations.
Generic SOCKS/HTTP TCP dials cannot reach this Unix backend. The pinned runtime's
Unix Serve PROXY-v1 plus explicit flow-cancellation contract is required; unmodified Tailscale cannot qualify it.
There is no AcceptedPeer constructor, deserializer or replacement-stream input.

The adapter reserves an opaque Producer-issued PendingStream before identification
starts. It retains only the weak cancellation handle, never a cloned payload
socket or caller-supplied UnixStream. The private backend origin is checked by
the Producer before identification. The genuine opaque AcceptedPeer's stream
must match the registered private original-accept cookie before replacing pending
storage. The same weak handle physically cancels payload and installed control
across that transition; retaining it cannot keep either descriptor alive.
The pending operation preserves the original 16 KiB send/receive buffer request
and 64 KiB observed maximum without exposing a descriptor to the Gate.
The same stream supplies original source IP, full DNS, tags and StableID; only then
may matching policy connect the literal loopback target. No metadata-to-peer path
exists. Direct wildcard sockets still use only their actual accepted peer CIDR,
including mixed policies; neither a named rule nor a forged header widens them.
The existing direct `Gate::bind` still refuses all named rules. TailnetOnly
remains unsupported: this adapter owns no workspace-membership authority.

## Geometry and original lifetime

Root must supply the externally owned original mapping. This adapter neither
creates, inspects, adopts nor removes Serve/Funnel configuration. P and V are
distinct ports within 20000–20031; B is exactly V+32 in 20032–20063. The Gate
exclusively binds IPv4/IPv6 wildcard P and accepts Unix streams only through the
consumed Producer's original listener. B selects that private socket filename;
there is no TCP B listener, loopback bridge, copied descriptor adoption or
caller-supplied listener. `producer_backend()` returns path comparison data only.
The existing root requirement, exclusive binds,
partial-bind refusal and protected target-port validation remain. There is no
physical 600–663 mapping, protected receipt/runtime consumer, public listener
activation, keeper, renewal or provider cleanup in this outcome. Unprivileged
high-port rebinding after release remains a separate protected-runtime concern.

Gate ownership retains the actual Producer. Close first physically shuts pending
and active streams, aborts and joins every flow, retires listeners, then consumes
the same Producer's bounded original-child retirement. An error is not a clean
acknowledgement. Drop cancels immediately and supplies no awaited clean proof.
A caller must retain external mapping cleanup custody and cancel on its loss.
Producer facts are checked at admission, not a new ongoing membership/tag-policy
subscription. No reconnect, stored observation or changed mapping is adopted.
Cancelling denies admission and physically shuts tracked Unix descriptors immediately;
awaited close also retires the Producer-owned listener and original socket path.

## Shared bounds

The private transport operations select TCP or opaque AcceptedStream readiness/read/write/shutdown;
whole cancellation of an identified flow always uses its private control owner, never payload-only SockRef shutdown.
Normal two-EOF completion and bounded budget drain retain that owner until the daemon's exact DRAINED `0x01` plus EOF.
They use only the original flow/grace deadline and cancellation; direct TCP finish remains immediate.
Control failure wakes connect, forwarding and grace even while payload I/O is blocked.
A Budget transition while awaiting normal finish preserves acknowledgement progress and enters the same fixed bounded grace.
State cannot mark graceful or release control on kernel acceptance of queued response bytes alone;
external payload and EOF remain separate actual-test requirements.
The same operations select TCP or Unix payloads;
there is no forwarding adapter or second connection. Main's CIDR/Public relay,
conservative pre-write charges, failed-write charges, byte grace, half-close, bounded draining, source history, deadlines, task-error
handling and early-drop cancellation remain the same implementation. Pending
identifications count toward total/provisional source limits and cannot reserve
payload bytes or attach a target. The Producer also retains its independent
16-identification and three-second bounds. Once identified, active counts and
retained source charges use the original StableID; direct traffic uses canonical
socket IP. All pending Unix identifications share one provisional bucket, with
no invented IP or pre-identity payload capacity. Its active count is released
on identification or cancellation, and the bucket stays bounded with the source
history. At most 256 accounting buckets remain, including the provisional bucket.
The immutable gate lifetime remains at most eight hours; there is no renewal API.

## Qualification boundary

Real TCP/Unix socket fixtures cover foreign pending-cookie substitution, retained
weak handles and physical cancellation across the pending/accepted transition, spoofed direct
metadata, mixed CIDR rules, pending caps, identification timeout/error, phase/
cancel and unpolled abort. Private State transport fixtures exercise Unix↔TCP
request half-close, actual response bytes after budget drain and Flow release,
immediate cancellation and pending-identity budget wake. These do not perform
named admission or create an identity. They construct only test-compiled transport/control wrappers, no successful AcceptedPeer, StableID, WhoIs or root identity.
All original Main Gate budget/grace/release/JoinError/cancellation tests remain.

The ignored actual selector requires an original root daemon/mapping and a
separate real tailnet peer in both Kernel and Worker/userspace modes. It must
show DNS-only and tag-only allows, named denial and mixed rules on the original
stream. A test-only observation counts only genuine opaque peers matching the
registered private accept cookie before policy evaluation and counts denial only when that flow also passes live
capacity/deadline checks; absence/custody/timeout/capacity refusal alone
cannot count as a passing named denial. Actual payload, forged-backend refusal,
half-close and final original shutdown/retirement are separate assertions.
No current actual result is claimed. See the [procedure](../testing/procedures/trusted-exposure-gate.md).
