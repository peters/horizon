---
applyTo: "crates/horizon-cloud-worker/src/trusted_tcp_producer/**"
---

# Original TCP producer boundary

Read docs/architecture/trusted-tcp-producer.md before reviewing this default-off component.
Report concrete inputs and wrong results for:

- A caller-selected path, decoded identity or PROXY header creating producer authority.
- Missing original UNIX credentials, pidfd, executable, process-start or namespace checks.
- An accepted stream from another Unix listener or process becoming an original Serve stream.
- A retained pending stream from retired backend A being identified by successor B
  at the same path. Require the exact private per-bind origin, not current path metadata.
- PendingStream exposing a constructor, raw descriptor/UnixStream, Clone or
  Deserialize before original control receipt; retained weak handles keeping
  queued SCM_RIGHTS alive after drop, unpolled cancellation or capacity refusal.
- Weak cancellation leaving the legitimate first-P control queued while the
  original pending owner remains retained, or receipt/install racing past cancel.
  Keep the one-byte discard bounded; malformed later rights need final owner drop.
- Pending cancellation losing the original payload/control when identification
  transfers to AcceptedStream, or original acceptance comparison using caller facts.
- A replaced backend path, writable ancestor, wrong permissions or stale listener becoming authority.
- A generic SOCKS or HTTP proxy in the original daemon forwarding an attacker-supplied identity header; same-daemon descriptor ownership does not prove Serve ingress.
- A changed daemon, Self identity, enrollment generation or backend passing a recheck.
- Header parsing before Unix peer and listener custody, application bytes consumed as header, a wrong virtual destination port or a query without finite bounds.
- Missing/extra/truncated/late-header SCM_RIGHTS, non-P prefix, unbounded ancillary allocation or payload overread passing receipt.
- A control FD with wrong family/type/unnamed addresses, flags, liveness or original root-daemon credentials passing admission.
- Cancellation, an unpolled future or capacity refusal leaving payload/control open, including with retained wrapper/reader Arcs.
- Payload write EOF dropping cancellation custody, or any public constructor/control-FD export replacing opaque AcceptedStream ownership.
- Treating control EOF without exact DRAINED0x01+EOF as success, losing interrupted ack progress, or acknowledging remote application delivery from queued bytes.
- Child cancellation returning capacity before actual kill/wait or retirement claiming clean completion after uncertainty.
- Retirement removing a replacement socket path or admitting a TCP backend reachable by generic proxies.
- A copied AcceptedPeer, replacement stream or serialized facts entering the authority path.
- This component activating Gate policy, Serve/Funnel, a keeper or public operations.
- Component evidence replacing required protected-runtime or complete Native/public M2 qualification.

Fixed runtime endpoints and root checks are component prerequisites.
They do not prove protected host publication, owner approval or workspace membership.
