# Retained exposure records

## Scope and trust boundary

Core defines bounded exposure intent and comparison facts.
The `cloud-exposure` feature is off by default and enables pure model transitions only.
Passive decoding is available with cloud workspaces.
Store and paired conversion retain exposure metadata without activating a runtime.
This prerequisite adds no listener, keeper, provider client, owner approval or public operation.

Serialized input is untrusted.
Strict decoding rejects unknown fields, duplicate nested keys and values that fail the model invariants.
A decoded value supplies no authenticated membership, transport custody or permission to reopen.
Validation bounds the decoded value, not allocations in a caller's JSON parser.
A caller must bound encoded input before decoding; this model adds no raw JSON dispatcher.

## Storage and versions

A present exposure record selects Deployment version 7 and paired Allocation version 5.
An absent exposure record keeps the existing version selection and encoding.
The highest version retains publication, enrollment, one-device admission and sibling metadata.
Store reads and writes, paired conversion and neighboring-worker checks recognize the same version.
An earlier version cannot carry exposure data; a highest-version record without exposure is invalid.

Deployment validation binds retained exposure to its cloud ID.
Paired conversion also requires the original session, workspace and cloud to match the project identity.
It preserves the allocation/project identity and dedicated-worker fences.
These checks validate comparison facts and do not grant provider or transport authority.

## Original comparison facts

A binding contains the original host, session, workspace, cloud, launch, worker and allocation specification.
It also contains pinned SSH arguments and a provider reference.
These are nonsecret comparison facts, not connection factories.
Nested launch and specification data must match the current writer's canonical encoding.
Unknown, duplicate or noncanonical data cannot disappear through decoding.
An API reference path must be absolute; the model does not read it.
A Docker host is validated as metadata for the Docker provider.

## Bounded service intent

A record contains one to eight services.
Service IDs and the currently retained operation IDs are unique.
Each intent has positive, ordered timestamps and a lifetime of at most eight hours.
Renewal changes only the selected service and uses a new operation.
It refuses clock rollback, overflow, an expired service or the wrong phase.
The original epoch and custody must match each transition.
Source and route validation use the merged Protocol policy.

The model keeps each service's latest operation, not every historical command.
Its uniqueness check is not a complete replay ledger.
A later live worker and controller must enforce their own bounded command history.
Metadata expiry is not a physical timer or proof of closed connections.
M1 sharing has no expiry.

## Uncertainty and authority

Unknown startup cannot become Observed or add a service through decoded data.
Finish retains its original operation through uncertainty.
The metadata transition to Closed requires that exact operation.
A decoded Closed value does not prove that a worker, process, provider resource or flow was retired.

Optional provider progress is passive evidence.
Its bounded rule text, action IDs and resource IDs supply no permission to repeat an effect.
Duplicate provider map keys and noncanonical typed key aliases cannot erase retained evidence.
The Closed shape requires absent provider progress when that field is present.
Actual absence still needs proof from the later original controller.

## Atomic delivery and module boundaries

The model and pure transitions were delivered separately behind the feature flag.
This outcome connects them to Store and paired Allocation persistence.
It reuses the existing unique-key JSON decoder within cloud runtime.
`binding.rs` validates comparison facts; `provider.rs` validates passive progress.
`lifecycle.rs` owns the feature-gated transitions.
The model's duplicate-provider and Finish/service operation checks remain unchanged.

The nine source/test leaves form one atomic persisted-format outcome.
The Deployment and Allocation schemas, version selection, readers, writers, conversion,
neighbor fence and required fixture initializers must agree to prevent rejected or lost state.
The existing UI fixture stores its larger Deployment array on the heap to satisfy the blocking stack-array lint.
A split that exposes only part of this format would leave an incomplete reader/writer contract.

One composite regression retains publication, enrollment, one-device admission and exposure
through the same Store and paired-format round trip.
Separate field tests do not establish that result.
The Store regression retains unresolved exposure without granting reopen authority,
and a rejected cross-cloud exposure save leaves the original durable bytes unchanged.
A version-7 neighbor regression exercises the public migration API: a shared worker
is refused, a different worker succeeds, and both preserve the original neighbor bytes.
Absent-exposure records retain the earlier formats and existing regression coverage.

Runtime, provider and public activation remain later outcomes in issue #1335.
These records do not deliver the complete M2 feature.
Use the [record procedure](../testing/procedures/exposure-records.md) for this outcome.
