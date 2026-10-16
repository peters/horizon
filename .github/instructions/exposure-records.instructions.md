---
applyTo: "crates/horizon-core/src/cloud_runtime/state.rs,crates/horizon-core/src/cloud_runtime/state/exposure/**,crates/horizon-core/src/cloud_runtime/allocation/legacy.rs,crates/horizon-core/src/cloud_runtime/allocation/legacy/unique_json.rs,crates/horizon-core/Cargo.toml"
---

# Exposure intent model boundary

Read `docs/architecture/exposure-records.md` before review.
This prerequisite defines bounded comparison data and pure transitions.
It does not change active Deployment, Allocation or Store wire formats.
The `cloud-exposure` feature stays off by default and starts no runtime operation.

Report these failures:

- A model value, path, UUID or SSH argument constructs a live client, approval or transport.
- Unknown, duplicate or noncanonical nested launch/specification data disappears during decoding.
- More than eight services, duplicate current IDs or an invalid route enters the model.
- A timestamp overflows, rolls back or permits more than eight hours.
- Renewal changes another service or extends an expired service.
- A stale epoch, custody, service port or operation advances a phase.
- Unknown startup reopens a service or decoded Closed data supplies physical cleanup proof.
- Provider progress authorizes an effect or claims actual provider absence.
- Current operation uniqueness is described as a complete historical replay ledger.
- M2 expiry is applied to M1 one-device sharing.
- This first prerequisite changes a saved format field, version selection or an existing persistence fence.

The later atomic persistence outcome must include the composite Store and paired-conversion regression.
Separate field tests do not prove retention of exposure, publication, admission and enrollment together.
