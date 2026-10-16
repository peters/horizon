# One-device Core owner controller

The optional `one-service-gateway` feature composes the endpoint engine, private
pinned worker SSH primitive and retained admission metadata in a Core controller.
The feature stays off by default. No UI, CLI or MCP caller invokes this controller
in this delivery. A future native integration must verify current saved membership
and obtain explicit owner approval; public requests can carry metadata only.
This controller alone does not deliver one-device sharing or real-owner evidence.

## Inputs and authority

Endpoint text, DNS, routes, worker replies and serialized records are untrusted.
A checked endpoint describes the current source and literal IPv4 host/port. An
`OwnerScope` describes host, session, workspace and cloud identity. Its constructor
checks syntax, not permission. Decoding either metadata or a receipt grants no
approval, filesystem authority or ability to restart access.

The trusted native caller must verify its current original persistent
session, workspace, cloud and agent membership, and obtain explicit owner approval
for the exact checked endpoint before starting the controller. Core cannot derive
that desktop membership from a saved record. No public request may supply that
approval, owner, Store, worker, SSH route or receipt.

`Recipient::checked` captures current original ready deployment facts and the full
native launch, including its placement and publishing intent. It refuses pending
replacement and session restart. Start compares the original cloud, revision,
profile/specification, worker ID/image/endpoint/Docker route and pinned SSH argument
vector from the held Store and current settings. It never follows a successor.
The SSH primitive retains strict host-key checking, the original key/known-host
paths and its fresh private Unix ingress. No credential is sent to a peer.

## Durable dispatch and physical custody

`Admitted` retains actual original Store custody through an opaque `OriginalCloud`
alias for its whole lifetime. The OS lock is held once; original controller aliases retain
the same original owner and worker. Serialized field transactions read fresh
state and preserve sibling admission, exposure, publication and enrollment fields.
An alias is not approval: each new operation still requires current native context
and the original explicit endpoint approval.
It refuses an unresolved Armed or Attached record. A Closed record permits only
a new explicit admission with newly checked endpoint and recipient facts.

The controller creates validated Armed metadata and saves it before starting any
listener or SSH helper. A failed save sends nothing. Later startup, save or thread
failure retains the original uncertain attempt. A record cannot recreate a gateway.

After dispatch, an independent cancellation watcher starts before Attached is saved.
It performs no deployment, settings or saved-owner read. Cancellation closes the
exact physical source even while the record observer blocks. Attached means local
custody; only the unchanged helper's strict Ready reply supplies its worker-loopback
proxy address. Neither Attached nor local gateway Active proves target reachability.

The record observer checks the original recipient and exact retained record.
The trusted native caller's `observe` also compares current OwnerScope, the full
launch and settings/pinned connection. Changed or unavailable facts revoke the
original source; they cannot transfer access to a new owner, worker or route.
A blocked observer keeps the original lock and uncertain metadata until it retires.

Close requires the exact original owner and operation/generation receipt. Stale
receipts cannot affect current access. `finish` borrows the handle, so an invalid
owner or receipt returns without consuming it or triggering Drop. Dropping the
owned handle is a separate retirement action. Physical revocation happens before any
observer or SSH join. `finish` reaps those owned resources, reads fresh state under
the same held Store, requires the exact original retained admission field, and only
then replaces that field with Closed. Sibling fields remain intact.
A missing/changed record or failed final save leaves uncertainty. Drop retires
physical resources but does not invent durable Closed.

## Scope and remaining delivery

The engine still checks the exact LAN or current kernel-tailnet source and connects
to its captured literal. Source drift closes old relays. Private SSH uses the
existing worker protocol and a fresh mode0700 directory/mode0600 Unix socket under
trusted root-owned sticky `/tmp`; its numeric receipt port has no TCP accept loop.
There is no new worker command, provider/API effect, enrollment, policy mutation,
credential grant, public ingress or expiry rule. The eight-hour rule belongs to M2.

The real dispatch calls explicit `Gateway::start_private`; ordinary `Gateway::start`
keeps its existing TCP behavior. Original Store custody requires the existing Unix
directory-durability support. Non-Unix hosts refuse Store acquisition before state
or provider effects, so they cannot construct a live original-owner alias. Private
physical fixtures run on Linux; Store-backed tests run on Unix. Pure generation
and admission-record checks stay portable. The existing non-Unix Store refusal
regression verifies unchanged record bytes, no new state folder and no operation lock. Passive sibling exposure-field tests use the
already merged record format, without a keeper, provider or public exposure route.

Colocated component tests cover durability, original lock custody, stale identities,
recipient drift, cancellation and physical retirement during a delayed record read.
They do not qualify a real owner, network, worker SSH or public interface.
The [controller procedure](../testing/procedures/one-device-owner-admission.md)
keeps those results separate. Completed M1 still requires native membership and
approval fulfillment, UI controls, existing MCP/CLI plan-runner parity, actual
LAN and tailnet sharing, failure/cleanup, and first/final native GIF evidence.
