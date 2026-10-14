# Tailnet API credential removal and status boundary

## Trusted authority

The Horizon host and its OS credential store are trusted.
The host store supports a personal API token or OAuth client for each saved tailnet.
The saved local tailnet ID names a binding, not proof of remote credential authority.
Profiles that reuse that ID intentionally share its API binding; a profile name
does not create credential isolation.
API credentials stay on this computer. Enrollment transports receive no API secret.

API credentials use the OS-store service `horizon-cloud-tailnet-api`.
Enrollment auth keys use the separate service `horizon-cloud-tailnets`.
API save, replacement and removal leave the enrollment key intact.
Auth-key replacement leaves the API binding intact.
Removing a saved tailnet cleans both bindings under the shared catalog lock.

## Durable binding changes

The API binding file and recovery journal contain nonsecret metadata only.
Each save writes a fresh opaque slot before it commits the binding.
Recovery discards the new slot before commit, or retires the old slot after commit.
Cleanup acts only on the exact recorded accounts of the saved local tailnet.
A failure cannot authorize cleanup of another tailnet's slot.
An explicit removal suppresses staged fallback, also after uncertain cleanup.
The disabled record remains while the saved tailnet still exists. After catalog
deletion commits, its recovery journal prunes only that removed tailnet's disabled
record. Metadata cleanup failures keep the deletion journal for retry. Active
bindings, pending API changes and sibling records cannot be discarded as part of
this pruning. Completed deletions do not accumulate permanent tombstones.

## Staged credentials

A saved tailnet without a managed API binding can use its staged OS-store entry.
That entry uses the API service and the exact saved local tailnet ID.
Personal-token JSON has `kind: personal_token` and `token`. OAuth JSON has
`kind: oauth`, `client_id` and `client_secret`. The personal token must start with
`tskey-api-`; an enrollment `tskey-auth-` key is not an API credential.
The resolver accepts bounded fields and refuses contradictory or unknown data.
It never searches another tailnet, namespace or enrollment key as a fallback.
A missing managed secret does not fall back to a staged older token.
Status reads do not migrate or rewrite a staged credential.
They can finish a prior recorded binding recovery, including retirement of the
exact obsolete OS-store slots. Missing and disabled metadata are different: an
explicitly disabled entry reports missing without trying staged fallback.

## Untrusted input and public output

Credential drafts and stored bytes require bounded validation.
Secret values use zeroizing storage and have no public read or debug API.
Public status returns only missing or stored credential kind.
Locked, unreadable or malformed storage returns a value-free error.
It is not proof that a credential is missing.
No secret enters project configuration, persisted application state, worker payloads or logs.
Callers must run filesystem and OS-store operations off the UI thread.
MCP must not accept raw API secrets or expose saved values.

## Status adapters

Settings > Tailnets displays API credential kind from background workers.
The native owner can remove an API credential independently of its enrollment
auth key. This outcome has no API entry or replacement control.
Filesystem and OS-store work remains off the render thread. Same-root Settings
close/reopen transfers the single IO owner, including pending read/write jobs,
barriers and their separate errors. Reopening does not start an independent
reader that can replace a pending write with an older stored status. While
Settings owns these jobs, production keeps a passive nonsecret catalog/status
mirror and cached cloud selections, without refreshing the store. Fresh reads
wait for retained operations; stale replies cannot restore a removed network.
A changed storage root blocks new editor actions and refreshes. Existing jobs
retain their original root and may finish; their state cannot authorize another
root. Discovery refuses the mismatch until Settings closes.

The CLI is `cloud_deploy tailnets STATE_ROOT [TAILNET_ID]`.
CLI and `cloud_companions.tailnet_api_credentials` share nonsecret observations
keyed by saved local tailnet ID. Ready contains missing/stored kind; pending and
unavailable do not imply missing. Stored kinds are personal_token and oauth.
MCP reads a storage-root-scoped background cache, polled without Settings.
While Settings is open, frame polling republishes its nonsecret observations to
that cache before MCP requests. Pending operations and their completed or failed
status become visible without closing Settings. Snapshots copy only when their
catalog/status revision changes. Closing Settings removes that override and
transfers unfinished barriers before the next fresh catalog/status read.
Known per-ID storage failures remain unavailable during that handoff and retry.
Only a fresh result for the current saved ID replaces the failure; removed IDs and
old successful snapshots do not survive the handoff.
A null catalog and empty map before bootstrap are not proof of missing credentials.
Saved local tailnet IDs identify bindings; profile names do not create new bindings.
Using one saved ID across profiles intentionally shares its API credential.

Status submits no new credential mutation or remote cleanup command. The resolver
can finish a prior recorded binding recovery, including retirement of its exact
obsolete OS-store slots. It does not migrate a staged value merely for status.
No CLI or MCP operation imports an API secret or exposes a saved value.
See [the status procedure](../testing/procedures/tailnet-api-status.md).

Worker enrollment remains auth-key based and requires a separately qualified
marker-4 image. API status does not update a worker pin.
Completed saved-tailnet deletion also removes its exact disabled API record.
The cache must not restore that removed ID from an older observation.

Stored presence means the resolver can read and parse the OS-store entry.
It does not test validity, expiry, selected scopes, tags or API reachability.
Saving does not contact Tailscale, mint a token or change remote access.
API credential management does not capture enrollment IDs or remove tailnet nodes.
Worker identity capture has its own [receipt boundary](tailnet-enrollment-receipts.md).

For an OAuth client, select `devices:core` and `tag:horizon-worker` at Tailscale.
Do not assume that tag selection restricts which devices the credential can delete.
Treat its device-removal authority as able to reach other tailnet devices.
Personal API tokens can have broad API authority and an expiry date.
Future cleanup needs a separate recorded-device-ID boundary and qualification.
See [Tailscale trust credentials](https://tailscale.com/docs/reference/trust-credentials)
and [OAuth clients](https://tailscale.com/docs/features/oauth-clients).


## Native owner requests

Saved tailnet credentials are desktop-global. Workspace actor authentication
allows an agent to submit and observe its own metadata request. It grants no
permission to remove the binding. The host derives the root and saved
ID/name from its own background catalog; public arguments contain no root,
keychain namespace, reference, credential bytes or owner-grant field.

`tailnet_api_credential` carries remove, status or cancel plus the exact saved ID and a caller-chosen UUID. The existing provider
usage queue binds dispatch and results to its live host and original agent panel.
The host retains a bounded volatile request registry. Submission only enters
pending_owner and does not open Settings. CLI MCP plans use this same route.
Standalone workers and builds without cloud-workspaces refuse it.

The person chooses the exact row in Settings > Tailnets. Remove requests require
Approve API credential removal on that row. Direct native removal also needs an
explicit owner click. Set and replacement operations are refused.
The registry rechecks request ID, root, saved ID/name, requested operation and
15-minute expiry before accepting the owner choice. Cancel and Decline affect
only pending requests. Running writes cannot be cancelled. The background
worker rechecks the saved catalog target and uses the existing Store transaction
lock and registered OS namespace. There is no new binding revision/CAS protocol;
this does not promise atomic comparison of catalog names with concurrent
owner-directed transactions. Exact saved ID determines the credential binding.

Requests are pending_owner, running, applied, failed, cancelled or expired.
Applied comes from a completed local Store transaction, never an inference
from matching credential kind. Failed or interrupted work is not proof of
rollback; existing recovery and nonsecret Refresh establish current status.
A running result channel remains in the host registry when Settings closes.
Terminal results remain readable for 15 minutes after the host receives them.
An old approval deadline cannot discard a newly received write result.
Status and cancel use only the original request's actor, root, saved ID and UUID.
They do not require the catalog row to survive a concurrent owner deletion.
New removal requests still require the host's current catalog target.
Read barriers and frame-based Settings observation still publish the actual
kind through CLI/MCP without requiring Settings to close.

Native owner UI and the desktop OS credential store remain trusted. This is not
a boundary against arbitrary same-user desktop code. No secret getter, CLI stdin
import, API entry, replacement, remote validation, enrollment receipt or automatic
node cleanup is supplied. The existing storage foundation can resolve an already
stored or staged credential; removal disables only the exact API binding and keeps
the saved network and enrollment auth key.

API status reads and native API writes keep separate error state. A successful
status read clears a reader failure but does not clear a failed write. A write
result cannot clear an unrelated reader failure or catalog/save error. Closing
Settings retains each pending job's kind and its failure. The parent status area
shows each error once; the API spinner stays in Settings. Request receipts still
report the actual Store result independently of these messages.

API credential request replies use the existing `HostIo` result worker in both
the cloud-enabled host and the feature-disabled refusal branch. Request validation
and volatile owner decisions remain on the UI thread; result-file publication
does not. Publishing a receipt grants no new binding or node-cleanup authority.
