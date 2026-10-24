---
applyTo: "crates/horizon-core/src/cloud_runtime/state.rs,crates/horizon-core/src/cloud_runtime/state/exposure/**,crates/horizon-core/src/cloud_runtime/allocation/legacy.rs,crates/horizon-core/src/cloud_runtime/allocation/legacy/**,crates/horizon-core/src/cloud_runtime/state/migration/companions.rs,crates/horizon-core/src/cloud_runtime/deployment.rs"
---

# Retained exposure record boundary

Read `docs/architecture/exposure-records.md` before review.
This outcome retains bounded comparison data through Deployment 7, paired Allocation 5 and Store.
An absent exposure record must retain the earlier formats and encoding.
The `cloud-exposure` feature stays off by default and starts no runtime operation.

Report these failures:

- A model value, path, UUID or SSH argument constructs a live client, approval or transport.
- Unknown, duplicate or noncanonical nested launch/specification/provider data disappears during decoding.
- A decoded Finish operation aliases a retained service operation in Finishing or FinishUnknown.
- More than eight services, duplicate current IDs or an invalid route enters the model.
- A timestamp overflows, rolls back or permits more than eight hours.
- Renewal changes another service or extends an expired service.
- A stale epoch, custody, service port or operation advances a phase.
- Unknown startup reopens a service or decoded Closed data supplies physical cleanup proof.
- Provider progress authorizes an effect or claims actual provider absence.
- Current operation uniqueness is described as a complete historical replay ledger.
- M2 expiry is applied to M1 one-device sharing.
- Exposure bypasses Deployment 7 or paired Allocation 5, or absent exposure changes an earlier format.
- A reader, writer, paired conversion or neighboring-worker fence omits the new highest version.
- The original session, workspace or cloud conflicts with the paired project identity.
- Saving invalid exposure replaces the previous durable state.

Require the composite Store and paired-conversion regression in this outcome.
Separate field tests do not prove retention of exposure, publication, admission and enrollment together.
