# Core recorded-node cleanup

This backend is behind the default-off Core `tailnet-node-cleanup` feature.
It requires `cloud-workspaces` to compile the cloud runtime. Default UI and CLI builds keep their existing behavior.
The final UI/activation outcome must enable cleanup through `cloud-workspaces`; this prerequisite does not activate it.


The existing owner cloud-delete controller deletes provider compute and managed storage before a node API request.
It holds the original cloud lifecycle lock throughout the operation.
It then sends only captured receipt IDs to the desktop NodeApi SDK.
Stop does not run node cleanup. Companion MCP stop and uncheck grants remain unchanged.
This change adds no public MCP delete operation or caller-supplied root authority.

The trusted desktop controller and root capture path supply ownership evidence.
The controller checks the cloud root, allocation request and original provider binding.
The retained receipt supplies the saved tailnet and opaque device ID.
Names, tags, peers, display labels and public inventory supply no deletion authority.
Deliberate metadata fabrication outside cooperating desktop writers is outside this guarantee.

Already-Deleted node cleanup retries read original durable provider records without a fresh provider GET or credential request.
A Prepared cloud without enrollment evidence retains its existing provider storage-finalization retry.
RunPod requires a matching Deleted storage journal when its required marker exists.
Legacy unmanaged RunPod storage can have no journal or marker.
Hetzner requires its completed deletion journal, terminated canonical volume ID, retained location, and no key or released server.
DockerHost requires the exact empty journal that its SDK saves after container and volume absence.
Missing required, malformed, foreign or unsettled resource evidence prevents API dispatch.

A receipt from another original provider remains manual.
Pending growth, resize, replacement or session restart cannot stand in for provider absence.
These checks depend on the existing trusted controller history and durable journals.
They do not supply a new per-receipt provider GET or authenticate an arbitrary library caller.
Real qualification remains necessary for every claimed provider lane.

Each saved-tailnet binding is processed independently.
Confirmation groups only the exact saved tailnet and device ID.
A retained node across worker replacement can have several worker-bound receipts.
A confirmed response marks that exact group together, without a second DELETE.
The same device ID under another saved-tailnet binding remains separate.
No tailnet name or address supplies an alias.

Unresolved attempts supply no device authority and receive no API request.
A valid known receipt can complete while an unresolved attempt remains.
Success preserves that attempt exactly and returns a manual identity outcome.
Unknown identity is never erased or confirmed because another receipt succeeds.
A selected legacy cloud without a captured ID also remains manual.
The same locked completion check fences redeploy before public-key validation, storage-journal release or readiness reset.
A deleted cloud with unresolved receipts, capture attempts or selected legacy missing identity cannot replace that evidence with a new allocation.
Fully confirmed receipts remain recorded through redeploy. A genuinely never-provisioned cloud or None without enrollment evidence can reopen.

The SDK resolves each exact desktop binding through the private credential Store.
Managed-slot loss, disabled bindings and failed recovery do not select staged fallback.
Credentials remain on the desktop; the worker receives no API token or OAuth secret.
HTTP 200 or 204 confirms deletion. Every 404 remains unconfirmed.
Missing, denied, unavailable or invalid API responses retain the exact receipt and return a fixed manual message.

Before dispatch, a cloned ledger must fit its confirmation within the existing encoded bound.
The controller saves confirmation before the next request.
A response without a successful save remains uncertain.
The receipt and its identity remain after confirmation; false flags keep the old encoding.
True flags cause older strict receipt readers to refuse decoding.
Current paired records preserve confirmation and reject duplicate keys, including escaped spellings.

The pass uses a shared 30-second HTTP budget.
The SDK limits each request to 15 seconds of the remaining budget.
Filesystem and OS-store operations keep their platform behavior, without a hard total wall-clock bound.
The first uncertain receipt ends the pass; later receipts remain pending.
The next owner retry uses the same record and pending groups.

With the feature enabled, existing owner Delete and cloud_deploy delete use the same Core entrypoint.
The shared `can_remove` check rejects pending receipts, unresolved attempts and selected legacy clouds without an ID
under the original lifecycle lock. This check and automatic API dispatch are enabled together.
An enabled backend cannot discard the original card through normal removal or automatic close while cleanup is pending.
With the feature disabled, Deleted retry and normal-removal behavior are unchanged; retained confirmation metadata still roundtrips.

This backend does not add pending-status presentation, a dedicated Retry node cleanup action or an explicit-discard warning.
Those UI changes and default activation are one subsequent outcome.
Existing explicit native RemoveAnyway remains deliberate and locked, with its original resource record preserved.
Default activation must supply its pending-node warning and preserve precise API/manual failure reasons during automatic close.
Actual native qualification and first-head/final-head real-input checks remain required before activation.
Current deployment diagnosis, captured projection, replacement handling and allocation version fences remain unchanged.
Set credential entry is inherited from the separate owner-editor prerequisite; this backend adds no entry or approval path.
