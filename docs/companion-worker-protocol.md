# Companion worker transport

This is the worker transport and discovery prerequisite for issue #910 M0.
Controller selection and automatic tool configuration are separate integration
work; these commands do not expose cloud lifecycle controls.

An owner invokes `horizon-cloud-worker companion-control` over the existing
authenticated SSH connection with one JSON request on stdin. It returns one
typed JSON response. The protocol supports:

| Operation | Effect |
| --- | --- |
| `identity` | Create or reuse a source-local Ed25519 identity for a grant; return only its public key. |
| `authorize` | Prepare a separate target worktree at an imported revision and install the source public key. |
| `connect` | Pin the supplied target host key, probe direct SSH access, then publish `companion-<alias>`. |
| `revoke` | Remove only the grant's authorized key; preserve worktrees and other access. |
| `disconnect` | Remove the source alias; retain the identity for target revocation reconciliation. |

Requests and responses are defined by `horizon-cloud-protocol::companion`.
Grant identifiers must be portable path components. Requests are bounded and
serialized on each worker. These operations never contact a cloud provider or
start, resume, stop, or delete compute.

The image needs OpenSSH, Git LFS, rsync, and the current worker helpers. Both
workers must already have their source objects and dependencies imported. The
target prepares submodules and LFS assets through the same helper as agent
worktrees, and records successful preparation before granting access. An
incomplete checkout requires explicit recovery; retries never reset existing
work. Initial Git checkout and source-material preparation each allow up to
300 seconds; authorization callers must allow those phases plus verification.
SSH readiness probes retain their shorter 30-second deadline. Worktrees live at
`/workspace/companions/worktrees/<grant>`.

Agents can run ordinary commands such as `ssh companion-app 'git status'` or
`rsync -az ./library/ companion-app:./library/`. The forced SSH entrypoint sets
the target's workspace home and starts in the grant's worktree. Private keys
stay on the source; agent forwarding and SSH connection multiplexing are
disabled. A failed connection probe does not replace a published host-key pin.

This grants trusted shell access as the target account, currently root. It is
not a sandbox or credential isolation boundary. Revocation blocks new SSH
authentication, preserves dirty work, and cannot undo copied data or terminate
an already established shell. Runtime grants are temporary; a controller must
reconcile selections after worker replacement or restart. Host identity changes
must be verified through the owner's authenticated connection.

## Read-only discovery

The owning controller publishes a bounded, validated catalog using
`horizon-cloud-worker companions publish` with JSON on stdin. Publication updates
discovery data only; it does not install grants, change connections, or start
compute. No catalog is inferred from repository files.

Agents use `horizon-cloud-worker companions list` or
`horizon-cloud-worker companions inspect <alias>`. The stdio MCP server,
`horizon-cloud-worker companions mcp`, exposes `cloud_companions_list` and
`cloud_companion_inspect` through the same implementation, plus the read-only
`cloud_offers`, which ranks compute offers from the prices the owning Horizon
last sent (`horizon-cloud-worker cloud-offers publish`). MCP receives no
provider credentials, endpoint, filesystem path, or lifecycle operation from
the caller. These tools require a current worker. Workers with agents advertise
the server as `horizon-cloud-companions` in each agent's MCP configuration.

List returns the controller's observation time, selection, repository/profile,
pinned cloud identity, status, and any existing SSH alias and separate worktree.
Ready observations expire after 60 seconds. Inspect checks an existing grant's
direct SSH connection and worktree and can verify access even when the controller
snapshot is old. It does not contact stopped, unselected, or unavailable targets.
Failures report unreachable, and selection changes during a probe require a
fresh inspection. Existing SSH can work while the controller is offline; a
snapshot's non-ready lifecycle state remains the last controller observation.
