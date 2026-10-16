# Exposure intent model

## Scope and trust boundary

Core defines bounded exposure intent and comparison facts.
The `cloud-exposure` feature is off by default and enables pure model transitions only.
The model is not connected to Deployment, Allocation or Store persistence.
Existing saved formats, version selection and neighbor fences stay unchanged.
This prerequisite adds no listener, keeper, provider client, owner approval or public operation.

Serialized input is untrusted.
Strict decoding rejects unknown fields, duplicate nested keys and values that fail the model invariants.
A decoded value supplies no authenticated membership, transport custody or permission to reopen.
Validation bounds the decoded value, not allocations in a caller's JSON parser.
A caller must bound encoded input before decoding; this model adds no raw JSON dispatcher.

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
The Closed shape requires absent provider progress when that field is present.
Actual absence still needs proof from the later original controller.

## Serial delivery and module boundaries

This first outcome defines the model and pure transitions behind the feature flag.
It reuses the existing unique-key JSON decoder within cloud runtime.
`binding.rs` validates comparison facts; `provider.rs` validates passive progress.
`lifecycle.rs` owns the feature-gated transitions; colocated tests exercise the model without Store.

The separate persistence outcome must update every Deployment and paired Allocation reader and writer together.
That outcome must retain publication, enrollment, one-device admission and exposure in one composite round trip.
It must also retain the project ownership and neighbor fences.
This first outcome adds no new saved format version or persistence field.
Runtime, provider and public activation remain later outcomes in issue #1335.

Use the [model procedure](../testing/procedures/exposure-records.md) for this prerequisite.
