# Core recorded-node cleanup

The final UI outcome activates the backend `tailnet-node-cleanup` feature through Core `cloud-workspaces`.
Existing UI default builds and owner `cloud_deploy` builds therefore share the same activation.
The feature adds no dependency back to `cloud-workspaces`, avoiding a Cargo feature cycle.
Confirmation metadata remains readable independently of this activation.


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
With cleanup enabled, Docker deletion preserves Prepared durably only if no container-port intent exists and the owned container is absent. Volume-only preparation does not imply enrollment. Any owned container found by the preliminary lookup, SDK removal lookup or final verification first saves Requested. Both SDK lookups use the same observation helper. A final-only observation stays recorded if the container disappears before retry; failed saves send no removal. Requested, port intent and legacy stop/resume/readiness history remain conservative even after resource absence. Synthetic Terminated ID shape never proves freshness.
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

Existing owner UI Delete and cloud_deploy delete use the same Core entrypoint.
Manage displays pending cleanup and supplies Retry node cleanup through the same owner Delete controller.
The existing sidebar and cloud_list read the same attention status; they grant no new delete operation.
Normal removal and automatic close recheck the saved ledger under the original lifecycle lock.
Pending receipts, unresolved attempts and selected legacy clouds without an ID retain their card and manual reason.

Explicit native Remove from Horizon anyway keeps its original deliberate-discard capability and lifecycle lock.
It warns that discarding the card preserves the original record but hides retry controls.
It issues no node API request or confirmation, and preserves the pending ledger.
Actual native qualification and first-head/final-head real-input checks remain required.
It preserves current deployment diagnosis, captured projection, replacement handling and allocation version fences.
Set credential entry is inherited from the owner-editor prerequisite; these cloud-delete changes add no credential entry or approval path.

Removal eligibility and recorded-node checks run on the existing background removal worker.
Display advice never authorizes discard: a fresh final check retains the original Store lock through exact-context UI application.
Cancellation, context drift and old generations cannot apply a later removal.
The UI preserves an earlier precise API failure only for the same displayed saved-tailnet/device pair; text supplies no authority.
A retained Ready connection watch does not prevent explicit RemoveAnyway, while busy operations and the lifecycle lock still refuse it.

Cleanup Retry reads saved settings on the background operation worker.
The card stays busy while that read waits. Cancellation checks run before the read and before cleanup dispatch. Invalid settings prevent provider and node work. A later Retry uses the same background read.

The UI keeps a display context for Retry from recorded pending cleanup or legacy missing identity. A failure alone never clears this context. Settings errors, cancellation, invalid launch identity and lifecycle lock contention retain Deleted, the current error and Retry through normal event polling. A legacy context also retains the manual missing-ID explanation when a later error replaces its text. An empty Deleted snapshot does not prove that legacy cleanup is complete.
A fresh Deleted snapshot with pending recorded receipts replaces Legacy display context with Recorded. It removes the obsolete legacy explanation and keeps Retry.
The first ordinary Remove that reports legacy missing identity establishes this context before another removal can replace its error. A subsequent busy removal retains the explanation and Retry. After provider deletion saves successfully, Core emits the exact saved Deleted snapshot while it still holds the original Store, before node cleanup can fail. Already-Deleted retries also emit their loaded snapshot before cancellation or cleanup. Cleanup then emits its last durable ledger on success or failure. A contended UI reload cannot lose the first pending receipt or retain an already confirmed receipt; failed saves supply no new deletion fact.

New authoritative lifecycle facts clear obsolete display context. These include a non-Deleted Stage, Ready, successful Deleted completion, Stopped or Resumed. A fresh snapshot also clears it when its stage is not Deleted or its recorded ledger confirms all node cleanup. The display context, cached snapshot and error text supply no deletion or removal authority. The original locked Store and current durable evidence remain the sole authority for Delete, node requests, normal removal and redeploy.
The explicit-discard warning also examines the raw saved-record reason, so a restored legacy cloud with no captured device ID receives it before any retry.
