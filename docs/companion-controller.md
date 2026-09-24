# Companion controller service

This M0 prerequisite connects passive selection to the worker protocol. The
checkbox UI and automatic tool advertisement are separate integration work.

`cloud_runtime::companions::refresh` takes the owning session/workspace and
source cloud identity, a trusted local inventory, an explicit selection action,
and machine-local SSH settings. Committed `.horizon/cloud.yml` provides available
declarations; it cannot authorize access. Inventory matches a GitHub origin and
profile and never discovers targets by panel title. Ambiguous identities require
an explicit binding. Targets in another owning workspace are excluded.

Selection is stored in the source cloud's private `companions.json` journal.
Stopped targets remain stopped. Reconciliation uses already-running workers and
has no create, start, resume, stop, or delete provider operation. Both deployments
remain locked during SSH setup, and the journal is saved before any grant can
be installed. The controller requires the existing owner SSH host-key pin.
Worker public keys cross the controller; private keys stay on their source.

The journal pins the selected cloud and its worker identity and initial target
revision. Retry reuses the grant and never resets its worktree. A changed
declaration, missing or ambiguous target, changed worker, or changed revision
cannot silently redirect access. Configuration errors prevent new grants while
allowing old grants to be revoked. Moving the source between owning workspaces
retires its previous grants before accepting new selections.

Unchecking records revocation intent first, removes source discovery/access,
and independently revokes the target key. Offline or uncertain cleanup remains
visible and is retried when the original workers are reachable. Selecting a new
target for the same alias requires completing that cleanup. Revocation preserves
worktrees, cannot undo copied data, and does not terminate existing shells.

Successful source-to-target SSH verification produces the shared discovery
catalog used by worker CLI and MCP. A failed refresh reports unreachable while
retaining a previously verified, unchanged connection for an independent worker
probe. Deselection and identity changes clear that connection information.
Catalogs include up to 64 declarations and 64 retained cleanup entries.

The service supports the same controller platforms as durable cloud state.
Unsupported directory durability, corrupt journals, busy operations, incompatible
worker images, and SSH trust/connectivity failures are explicit errors. No
network overlay or relay is installed. These clouds share trusted shell access
at the target account's privilege level; this is not credential isolation.
