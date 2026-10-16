# Exposure control comparison contract

The Protocol `exposure-control` feature is off by default and implies the existing
`exposure-gate` policy feature. It adds pure request/reply, member and Worker
observation types. It adds no transport, root process, listener, keeper, provider
operation, owner approval, persistence format or public UI/CLI/MCP activation.

## Untrusted wire input

Use `Request::from_json` and `Reply::from_json` for raw frames. Requests have a
20 KiB byte limit and replies a 144 KiB limit, checked before deserialization.
Malformed JSON, trailing values, unknown fields and duplicate known object fields
are refused. Typed values can also be deserialized directly, so a consumer must
still call validation; plain serde decoding does not apply the frame-size gate.
These methods bound supplied slices, not a transport's line reader or allocation.
A future transport must enforce its own bound while reading.

Version 3 sessions carry an allocation and non-nil epoch/challenge IDs. Requests
separately retain custody, command and service IDs. The contract supports Open,
Renew, Close, Read and Finish shapes. Valid syntax does not prove that the sender
holds the original stream, that a command was approved, or that an ID belongs to
an existing attempt. A live controller must compare the exact original binding
and request. There is no runtime dispatch in this component.

## Policy and time facts

Open validates the complete shared policy and explicit route and refuses Off.
The shared validator also rejects all protected physical listener/backend targets
600–663, alongside management targets and logical reserved targets 20000–20063.
The existing canonical ingress helper supplies this deny range; no port mapping
or listener is created. This one shared-policy restriction applies to every caller.

Lifetime facts contain worker elapsed-time anchors and remaining intervals.
Remaining time must be positive and at most eight hours, and addition cannot
overflow. This is arithmetic validation, not an expiry clock or renewal grant.
The future original keeper must compare its monotonic observations. M1 one-device
sharing keeps its existing no-expiry contract.

## Passive observations

Replies contain at most eight service entries. Duplicate service, operation or
ingress identities, invalid active/terminal intervals, inconsistent Ready/Closed
shapes and incompatible route/URL/ingress facts are refused. An entry retains its
last service-specific operation separately from a Finish reply ID. A Closed reply
is comparison data; it proves neither actual socket closure nor provider release.
The 4,096 mutation constant does not implement a replay journal.

Worker observations distinguish Unavailable, Running and Unverified. Canonical
Self.ID/DNS/suffix/address facts are bounded and validated. Running is serialized
comparison data, not an authenticated daemon observation. Unverified cannot supply
Ready or Applied, and losing a live daemon must never become an Unavailable
fallback. A future consumer must enforce the original observation chain.

TailnetOnly Gate requests require recipient-bound member comparison facts. There
are at most 64 strictly sorted unique stable IDs, including the exact recipient.
Other policies cannot carry these facts. A decoded member vector, matching DNS,
address, tag or UUID does not establish live workspace membership or authority.
No tailnet query, enrollment or admission happens here. HTTPS route facts likewise
supply no TLS process, Funnel publication or raw TCP fallback.

## Modules and qualification

`control.rs` owns frame shapes and complete validation. `control/members.rs`
owns bounded recipient comparisons, `control/worker.rs` owns passive observation
shapes and `control/tests.rs` covers raw frames and transitions.
The [procedure](../testing/procedures/exposure-control.md) is offline. Actual
transport custody, root/runtime checks and public feature qualification remain
separate outcomes.
