# One-device admission records

This storage prerequisite retains one-device attempt metadata for issue #1335.
It adds no owner decision, gateway constructor, worker command or public sharing operation.
The schema uses the existing `cloud-workspaces` feature and has no dependency on the endpoint engine.
Readers must retain the evidence even when a future runtime feature is disabled.

## Untrusted input and authority

Saved JSON is untrusted input.
The private `Fields` decoder checks owner identities, attempt UUIDs, endpoint fields and phase values before it returns a `Record`.
Unknown fields, nil identities, oversized values, invalid bindings and an attached phase with port zero fail.
A deployment must keep the same cloud ID as its retained owner scope.
Paired records must keep the same session, workspace and cloud membership.

The record fields are private.
There is no public `Record` constructor, phase mutator or restoration factory.
`OwnerScope::new` checks the shape of scope facts; it does not approve access.
Decoded metadata, IDs, endpoints and saved phase values grant no runtime authority.
They do not prove current membership, a live source, worker reachability or completed cleanup.

A future owner controller must check current facts and get explicit owner approval before an effect.
It must preserve the original attempt when its result is uncertain.
This schema does not supply that controller or its permission.

## Versions and retained evidence

A nonempty admission field selects deployment version 6 and paired allocation version 4.
The field is omitted when it is absent.
Absent admission keeps the existing deployment and allocation version rules.
Enrollment, publishing, sibling and working-copy evidence keep their existing meanings.

The reader rejects an old version that carries admission evidence.
It also rejects a new admission version without a valid record.
This fence prevents an older reader from ignoring and then erasing the attempt.
Armed, Attached and Closed records survive storage and lossless split/join conversion.
Removing the current worker does not erase the original attempt.

Publishing validation still applies when admission selects the newer version.
The existing publication, profile choices and account metadata remain unchanged.
An invalid publication cannot replace the saved deployment.

## Shared reads and module boundaries

`state::service_admission` owns the validated metadata types.
`state` selects the deployment version and checks the cloud binding.
`allocation::legacy` retains the complete record and checks paired ownership.
`state::migration` preserves the field beside enrollment and storage companions.

`Store::load` still delegates to `Store::snapshot`.
The snapshot still uses the shared record reader and refuses pending storage recovery.
It creates no directory, takes no operation lock and reserves no operation.
Execution must acquire the original lock and check the current record again.

The deployment initializer sets the new field to `None` without replacing current publishing preparation.
Existing UI test fixtures receive only the required initializer field.
No visible UI behavior changes.

Use the [permanent procedure](../testing/procedures/one-device-admission-records.md) for parser, version and migration checks.
Real owner, LAN, tailnet, worker SSH and public parity qualification belong to later outcomes.
