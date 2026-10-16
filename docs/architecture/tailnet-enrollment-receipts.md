# Tailnet enrollment receipt storage

## Current behavior

The core stores an enrollment ledger with each deployment record.
Selected deployment and reconnect operations fill the ledger from the private root daemon.
An attempt is saved before enrollment and the observed Self.ID before readiness.
This outcome adds no Tailscale API request, node removal or public operation.

A receipt contains the saved tailnet, provider, worker, allocation request and device IDs.
A capture attempt contains the same local identities and its own operation UUID.
An attempt has no device ID and supplies no node-removal authority.
The device name remains a separate display field.
No conversion creates a device ID from a device name.

## Storage boundary

The locked host record and pinned root capture caller are trusted.
Persisted metadata can be malformed and must pass the ledger schema.
Typed identities reject empty values, invalid local IDs and oversized fields.
Device IDs are opaque visible ASCII strings with a 256-byte limit.
A future API adapter must encode each device ID as one URL path segment.
The schema accepts no auth key, API token, client secret or arbitrary field.

The ledger has at most 128 entries in total and a 64 KiB encoded limit.
The decoder limits each array as it reads its entries.
Field and encoded-size checks apply to the decoded ledger.
These checks do not bound the existing deployment file reader before JSON parsing.
They do not prove that a device ID belongs to the saved tailnet.
They do not protect against a trusted caller or compromised worker that supplies a false ID.

## State transitions

The ledger keeps receipt and attempt arrays private.
The core supplies `arm` and `record` operations.
Both operations leave the ledger intact after a bound or conflict error.
An exact repeated attempt is idempotent while it remains armed.
A reused attempt ID with different identities is refused while it remains armed.

`record` requires an armed attempt.
It removes only that attempt and adds its observed device ID.
An exact repeated receipt does not add another entry.
A different device ID remains beside the old receipt, including on the same worker.
Other unresolved attempts remain intact.
The ledger never evicts an entry to make room.

The caller holds the cloud ownership lock and saves the complete deployment record.
The existing store syncs the file and directory through an atomic rename.
The capture caller saves the attempt before enrollment.
It saves the receipt before it permits sessions or Ready.
The store automatically writes deployment version 4 for any nonempty ledger,
unless a retained publication record requires version 5. Version 5 keeps both records.
This includes unresolved attempts and records with siblings.
Older readers refuse this version instead of saving away unknown evidence.
Current readers refuse evidence on an old version or an empty ledger on version 4.
Empty historical records keep version 1, or version 3 when they have siblings.

## Lifecycle and conversion

Old deployment records start with an empty ledger.
Empty ledgers are omitted from saved records.
Explicit empty metadata has the same meaning during legacy conversion.
Malformed or unknown ledger fields are refused.

The legacy conversion keeps the ledger with the allocation, which owns the worker.
An allocation with enrollment evidence uses version 2; an empty allocation keeps version 1.
A retained publication record requires allocation version 3 and keeps enrollment evidence.
The current paired-record reader refuses a mismatch between this version and the ledger.
The project record does not own the node identity.
Conversion back to the deployment retains all receipts and attempts.
Worker deletion and redeploy retain the ledger when they clear the worker field.
Provider-stage changes and an absent worker do not erase its identities.

Removing a cloud card keeps its saved record on disk under the existing close contract.
This change adds no inventory for removed cards and no cleanup action.
Later cleanup must retain uncertain receipts and expose manual outcomes through all supported interfaces.
An interrupted enrollment can create a node before the host records its ID.
An unresolved attempt retains that uncertainty without inventing a device ID.

See [the storage test procedure](../testing/procedures/tailnet-enrollment-ledger.md).

The receipt also retains the exact provider allocation request ID; it is independent of the UUID of a capture attempt. The ledger remains in the owning cloud's locked deployment/allocation record, under that Horizon cloud root. It does not create a new owner, grant or API credential. The capture operation verifies the current provider worker against the saved WorkerSpec, retains this request ID, and uses the saved tailnet ID. Moving or fabricating metadata does not grant deletion authority.

Typed manual cleanup reasons distinguish an unavailable device identity, a missing desktop API credential and an offline API. Their messages describe retry/manual cleanup while the ledger retains the recorded ID. This capture outcome neither calls an API nor enables a cleanup button or public status operation. No API secret belongs on the worker.
