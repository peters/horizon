# Original Hetzner exposure management boundary

The existing default-off Cloud `exposure-firewall` feature includes a management
controller. No default, provisioning, UI, CLI or MCP route starts it. Its two
production leaves depend only on the existing Hetzner SDK. They require no
Producer, worker control protocol or Core owner implementation.

## Original client and comparison facts

`Hetzner::prepare_exposure_management` consumes the original authenticated client.
The non-deserializable `Manager` retains that client, exact allocation/server
scope, original SSH endpoint, full foreign firewall baseline and all live service
contributions. Neither `Scope`, a saved journal nor serialized firewall facts
creates native approval, a restored controller or a new credential binding.

Responses are untrusted. A snapshot checks the original running allocation's
server, requires an explicit settled attachment list and reads every exact numeric
firewall. Duplicate IDs, transitions, selectors, malformed rules and a full five
attachment slots refuse. Attachments and SSH endpoint are read again after the
firewall reads. Unknown attachment fields refuse in enabled builds; default builds
retain their previous response compatibility. Existing numeric CIDR validation
and directional effective-prefix limits remain shared with the lower-level SDK.

## Management policy and mutations

On a server without firewalls, the caller supplies the actual approved management
rules, including an explicit inbound TCP allowance. The type validates rule shape;
it cannot establish that the rules cover the owner's actual pinned SSH endpoint.
The final owner must check that relationship before effects. When foreign
firewalls exist, the manager adds no management allowance and never rewrites their
inbound or outbound rules. It refuses foreign baseline drift instead of adopting it.

Every replacement includes management and all retained sibling contributions.
Intent is saved through the caller's original durable `Journal` before mutation;
returned resource/action IDs are saved before waits. Creation is one unattached
POST followed by exact apply and fresh readback. Uncertainty blocks service
mutation and successor creation. Action success alone is insufficient. Provider
preflight and mutation are not atomic, so another actor can race between requests.
A failed current-state verification latches uncertainty even if later replies match
the baseline. Only original-resource cleanup remains available. The requested create
rules become the retained expectation before any fallible action save or wait.

The caller must physically close its original worker Gate on every error and
before removing ingress. The SDK owns no Gate and does not prove physical closure.
Removing one contribution preserves siblings. A live epoch may retain its original
firewall with management-only rules for another service. Explicit final cleanup
removes/deletes only the original resource, or reconciles a lost-create response
under the same original scope and expected complete rules. It never changes rules,
reopens a service or adopts a successor during cleanup. A new create clears the previous
resource ID only after its own durable intent succeeds. A failed intent retains the
previous exact cleanup identity; a lost new reply uses the new scoped intent.
Cleanup uncertainty remains
uncertain; a retired manager cannot reopen. Dropping a manager does not prove cleanup.
Before cleanup adopts a fresh snapshot, it must match the original SSH endpoint and
foreign baseline. Its owned firewall must retain the rules validated by the initial
lookup. The journal records that fresh captured firewall, including its current
attachment, before retirement. Absence cannot authorize a changed SSH endpoint.

## Qualification limits

The permanent [procedure](../testing/procedures/exposure-management.md) separates
loopback algorithm tests from actual provider checks. No real management-access,
source-preservation, public readiness, worker expiry or provider-release proof
follows from an SDK response. Hetzner rules do not filter private-network traffic
or terminate established connections; physical worker enforcement remains required.
RunPod observed mapping support and its unqualified source/isolation lanes are
unchanged. Full #1335 still requires its actual owner and public interfaces.

## Ignored original-provider test

The Core Hetzner test tree contains one explicit ignored SDK consumer. The existing
Core `cloud-exposure` feature forwards SDK availability through `exposure-firewall`;
defaults and public dispatch remain unchanged. This test uses current Main Store,
normal Hetzner Journal, Settings and pinned Connection. It needs no future exposure
owner or Producer implementation. The test never writes normal deployment/provider
records or enrolls a tailnet.

A separately approved supervisor must first create one normal SSH-only worker with
Tailnet=None. Its fixed private input records the exact original identities, input
hashes, credential/key metadata and actual SSH management source. These facts do
not grant Native approval. The test holds the original Store lock, checks current
provider ownership and volume/key binding, then consumes the actual SDK client.
It refuses unexpected attachments instead of mutating existing foreign firewalls.
A bounded exclusive journal exposes scope before POST and returned IDs before waits.

Every operation uses the same Manager. Result-returning work always attempts original
cleanup before the test reports an error. A process crash cannot restore that Manager;
the original supervisor must retain the exact journal and arrange reviewed cleanup.
An unattached firewall can survive ordinary worker deletion. This test alone does
not qualify service bytes, source attribution, physical expiry or any public route.
