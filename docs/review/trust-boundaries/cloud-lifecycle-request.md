---
applyTo: "crates/horizon-ui/src/app/cloud_panel/production/lifecycle_agent.rs,crates/horizon-ui/src/app/cloud_panel/production/lifecycle_agent/**,crates/horizon-browser-control/src/manifest/cloud_lifecycle.rs,crates/horizon-browser-mcp/src/controller/cloud_lifecycle.rs,crates/horizon-core/src/cloud_runtime/lifecycle.rs,crates/horizon-core/src/cloud_runtime/lifecycle/**"
---

# Cloud lifecycle request boundary

Read the cloud lifecycle test procedure
`docs/testing/procedures/cloud-lifecycle-tool.md`, the create boundary
`cloud-create-request.md` and the delete boundary
`cloud-delete-request.md`.

## Untrusted input

- An agent panel of a workspace. Its `cloud_lifecycle` request is untrusted: the
  operation, the action, the cloud ID and the caller-chosen operation ID.
- The saved record of a cloud, which carries the creator the lease reads and the
  stage the answer reports.
- The answer of a provider during the stop or the resume.

## What the host proves before the provider call

- The request belongs to this Horizon instance and to a live agent panel, in the
  workspace of that panel. Another host instance, or an actor without an agent
  panel, is refused before any file or provider call.
- The cloud ID names exactly one cloud in that workspace.
- No other operation owns the cloud: the card's own deployment, provider check
  or preparation is `cloud_lifecycle_busy`, and so is a running create operation
  of that cloud, because a power action must not fight one. A running lifecycle
  operation of the same cloud is `cloud_lifecycle_busy` too.
- The lease: the cloud's record names the agent panel that created it, and only
  that actor stops or resumes it. A cloud with no creator was made by a person
  and is refused, and the message says the card acts on it.
- The action matches the record: a stop of a cloud that is already stopped sends
  nothing and answers `stopped`, and a resume of a cloud that is not stopped is
  refused with the stage it found. A record that names no worker has nothing to
  stop or resume.
- The stop and the resume then take the Store lock as the card's own actions do,
  so a cloud that another controller holds is `cloud_lifecycle_busy` and no
  provider request starts.
- One audit entry records each answered call with the actor, the cloud, the
  action, the phase it reached and whether it failed. It holds no credential,
  provider token or worker ID.

## What it does not guarantee

- A stop and a resume are irreversible once their provider request leaves this
  computer, so the tool has no `cancel` and a lost answer is read with `status`.
  A repeated `submit` with the same operation ID answers the operation it
  already has and sends no second provider request.
- What a stop keeps is the provider's decision, not Horizon's: `kept` names it
  from the provider's description, and a resume finds the same workspace volume
  only on a provider that keeps one. A provider that bills a stopped worker
  still bills it, and this tool does not make it free.
- A stop ends compute only. It deletes no resource and proves no absence; that
  is `cloud_delete`.
- The lifecycle functions read the cloud's record themselves, under its own
  lock, after this boundary dropped the lock it read the lease with. A
  deployment that starts in that moment is stopped or resumed with the cloud.
- Another process may hold the Store lock. That is a failure the answer names,
  not a wait, and the retry is the caller's.
- The operation table is in memory. After a Horizon restart the operation ID
  answers `cloud_lifecycle_unknown_operation`, and only the cloud's record says
  what the worker's stage is.
