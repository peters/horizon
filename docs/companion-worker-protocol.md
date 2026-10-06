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
| `revoke` | Remove the grant's authorized key, then its worktree and prepared record when the worktree has no changes and no untracked or ignored files; a dirty worktree and other access are preserved. |
| `disconnect` | Remove the source alias; retain the identity for target revocation reconciliation. |
| `forget` | After the target confirmed `revoke`, remove the disconnected grant's key directory on the source. Refused while the grant is still connected; older worker images refuse it, and their key stays until the container restarts. |

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

## Agent access

On a worker with agent isolation, agent and shell sessions run as the agent
user `horizon-agent` (UID and GID 10001). The root files under
`/run/sshd/companions` stay private to root. The source publishes copies for
the agent user in `/run/horizon-companions`:

| Path | Owner and mode | Contents |
|---|---|---|
| `/run/horizon-companions` | root, group 10001, `0750` | The directory of the copies. |
| `config` | root, group 10001, `0640` | The `Host companion-<alias>` blocks of the connected grants. |
| `<grant>/identity` | root, group 10001, `0640` | A copy of the private key of the grant. |
| `<grant>/known_hosts-<key>` | root, group 10001, `0640` | The host-key pin of the target. |
| `<grant>/connection.json` | root, group 10001, `0640` | The alias and the worktree of the grant. |
| `catalog.json` | root, group 10001, `0640` | The discovery catalog. |

OpenSSH reads the user file from the home directory in passwd, not from
`HOME`. For this reason, the source writes `/etc/ssh/ssh_config.d/horizon-companions.conf`.
This file includes `/run/horizon-companions/config` only for the agent user
(`Match localuser horizon-agent`). The SSH client of root does not apply the
copies. The agent user can read the copies but cannot change them.

Before `connect` reports Ready, the source runs the readiness probe a second time
as the agent user. If that probe fails, the source removes the alias and reports
an error. `disconnect` removes the alias, the key copy and the pin copy for the
agent user. If the source cannot copy the files of a grant, it removes that copy
and reports an error. `forget` also removes a copy that remains after an
interrupted disconnect. Catalog publication refreshes the copies first. If that
fails, the source removes the catalog and reports an error. A worker without
agent isolation runs agents as root and publishes only the catalog.

An agent session can copy the key. A copied key works until the target revokes
the grant. Only target revocation blocks a copied key.

This grants trusted shell access to all agent sessions on the source. On the
target, the forced entrypoint runs the shell as the agent user of an isolated
target, or as root on an older image. It is
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
It waits up to 10 seconds for grant setup that holds the companion lock, as the
owning Horizon's refresh does briefly; a lock held throughout reports the access
as unverified, never unreachable. The agent user cannot take the companion lock.
Its inspection reads the agent copy of the connection record before and after
the probe. If the record changes, the access is unverified. Other failures report unreachable, and
selection changes during a probe require a fresh inspection. Existing SSH can work while the controller is offline; a
snapshot's non-ready lifecycle state remains the last controller observation.
