---
applyTo: "crates/horizon-core/src/cloud_runtime/tailnet/receipt.rs,crates/horizon-core/src/cloud_runtime/tailnet/capture.rs,crates/horizon-core/src/cloud_runtime/tailnet/capture/**,crates/horizon-core/src/cloud_runtime/state.rs,crates/horizon-core/src/cloud_runtime/allocation/legacy*,crates/horizon-core/src/cloud_runtime/allocation/legacy/**"
---

# Enrollment capture and storage boundary

Read `docs/architecture/tailnet-enrollment-receipts.md`.
Also read `docs/architecture/tailnet-receipt-capture.md`.
Selected deployments/reconnects capture Self.ID and persist bounded nonsecret receipts.
API deletion and its UI are the next separate outcome.

Check these storage properties:

- Empty historical records have no device-removal authority.
- Only typed local IDs and opaque bounded device IDs enter the ledger.
- Bounds and conflict errors preserve all prior evidence.
- A changed device ID never overwrites an earlier receipt.
- Recording clears only the matching capture attempt.
- Clearing a worker, deleting or redeploying the cloud preserves evidence.
- Legacy conversion keeps evidence with the allocation and round-trips it.
- Nonempty evidence automatically fences old readers with a new deployment and allocation version.
- New versions without evidence and old versions with evidence are refused on load.
- An unknown field or duplicate ledger entry is refused.

The enabled capture must also satisfy these properties:

- The host saves the attempt before configure and the receipt before display or sessions.
- The pinned root reader uses fixed official CLI/socket paths and the existing control lock.
- Selection, Running state, sole worker tag and Self.ID are all checked.
- The reader bounds stdout before JSON parsing and suppresses raw status/errors.
- Symlinks, unsafe modes, wrong ownership, duplicate fields and process deadlines refuse capture.
- Root/image compromise is outside the declared authority guarantee.
- API credentials never reach the worker; capture deletes no node.

Ledger metadata does not prove remote ownership.
The existing full deployment reader is not a bounded streaming reader.
No credentials, peer list, device name or address is deletion authority.
