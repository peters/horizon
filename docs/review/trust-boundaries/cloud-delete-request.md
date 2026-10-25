---
applyTo: "crates/horizon-ui/src/app/cloud_panel/production/delete_agent.rs,crates/horizon-ui/src/app/cloud_panel/production/delete_agent/**,crates/horizon-browser-control/src/manifest/cloud_delete.rs,crates/horizon-browser-mcp/src/controller/cloud_delete.rs,crates/horizon-core/src/cloud_runtime/state.rs,crates/horizon-core/src/cloud_runtime/deployment/deletion.rs"
---

# Cloud delete request boundary

Read the cloud delete test procedure `docs/testing/procedures/cloud-delete-tool.md`,
the create boundary `cloud-create-request.md` and the removal boundary
`cloud-removal.md`.

## Untrusted input

- An agent panel of a workspace. Its `cloud_delete` request is untrusted: the
  operation, the cloud ID and the caller-chosen operation ID.
- The saved record of a cloud, which carries the creator the lease reads and the
  stage the answer reports.
- Every deletion journal of the provider, which is the only evidence that a
  provider resource is absent.
- The answer of a provider during the delete.

## What the host proves before the provider call

- The request belongs to this Horizon instance and to a live agent panel, in the
  workspace of that panel. Another host instance, or an actor without an agent
  panel, is refused before any file or provider call.
- The cloud ID names exactly one cloud in that workspace.
- No other operation owns the cloud: the card's own deployment, provider check
  or preparation is `cloud_delete_busy`, and so is a running create operation of
  that cloud, because a delete must not fight it.
- The lease: the cloud's record names the agent panel that created it, and only
  that actor deletes it. A cloud with no creator was made by a person and is
  refused, and the message says the card deletes it. A cloud whose record
  requests no worker has nothing to delete and is refused.
- The deletion then takes the Store lock as the card's own delete does, so a
  cloud that another controller holds is `cloud_delete_busy` and no provider
  request starts.
- One audit entry records each answered call with the actor, the cloud, the step
  it reached, the provider and whether absence was proven. It holds no
  credential, provider token or worker ID.

## What it does not guarantee

- A delete is irreversible once it is accepted, and a provider request that left
  this computer cannot be recalled. A cancel answers that the delete may already
  have started; it never claims that it stopped one. A cancelled operation keeps
  `done: false` until its deletion thread reports, and that report then records
  the step and the absence outcome for the cancelled operation too, because they
  describe the cloud and not the request.
- Absence is only as good as the provider's own confirmation, recorded in that
  provider's journal. `absent: true` means the record and the journal agree that
  nothing the cloud created remains. `absent: false` means the records do not
  confirm it, and the answer says which case it is. It is not a fresh provider
  query, and it is not a promise about a resource the record never named. A
  record that another operation holds for the moment of the first read is read
  once more, and after that it is reported as unproven.
- The creator is an optional field of the cloud record, which the legacy split
  keeps. An older Horizon build ignores it when it reads `deployment.json`, and
  its own record format omits the field. A build that reads the split
  `project.json` instead denies unknown fields, so it rejects a split record
  that carries the creator rather than reading it without the lease.
- The record is read under its own lock, which is dropped before the deletion
  takes that lock again. A deployment that starts in that moment is deleted with
  the cloud.
- Another process may hold the Store lock. That is a refusal, not a wait, and
  the retry is the caller's.
- This tool deletes provider resources only. It removes no card, deletes no
  credential and keeps no discard authority: the person removes the deleted
  cloud with the removal flow that `cloud-removal.md` governs.
- The operation table is in memory. After a Horizon restart the operation ID
  answers `cloud_delete_unknown_operation`, and only the cloud's record and the
  provider's journals say what was deleted.
