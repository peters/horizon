---
applyTo: "crates/horizon-core/src/cloud_runtime/cloud_exec.rs,crates/horizon-core/src/cloud_runtime/cloud_exec/**,crates/horizon-core/src/cloud_runtime/command.rs,crates/horizon-browser-control/src/manifest/exec.rs,crates/horizon-browser-mcp/src/controller/exec.rs,crates/horizon-ui/src/app/cloud_panel/exec.rs,crates/horizon-ui/src/app/cloud_panel/exec/**,examples/cloud-worker/horizon-worker-tailnet"
---

# Trust boundary of a cloud worker command

An agent asks Horizon to run one command in the worker of a cloud. Horizon sends
the command over the cloud's recorded SSH connection and the worker's own
launcher runs it as the unprivileged agent user. The agent tool contract is
`cloud_exec` in `crates/horizon-browser-mcp/src/server.rs`.

## Trusted

- The person's Horizon, its settings and its cloud records.
- The host key that Horizon pinned for the worker, in the cloud's own directory.
- The SSH identity file that the person's settings name.
- The worker image and its launcher
  `examples/cloud-worker/horizon-worker-tailnet`.

## Untrusted

- **The command.** It is arbitrary code. Horizon does not parse it, allow-list
  it or restrict it.
- **Everything the command prints.** Both streams can hold control sequences and
  text that looks like the account frame.

## What Horizon guarantees

1. **The account is the agent user.** Horizon accepts no user, no uid, no
   environment and no shell flag. The remote text checks that it arrived as
   `root`, then hands the command to
   `horizon-worker-tailnet agent`, which refuses a caller that is not `root` and
   execs `setpriv --reuid=10001 --regid=10001 --clear-groups --no-new-privs
   --bounding-set=-all` with `HOME=/workspace/home`. The dropped process prints
   its own `id -u` in a frame line before it runs a single byte of the caller's
   text, and Horizon refuses the result unless that value is `10001`.
2. **No fallback to `root`.** The remote text exits with its own code when the
   worker has no `/run/horizon-tailnet/agent-isolation` marker or no launcher.
   Horizon reports a typed refusal and runs nothing. It never uses the fallback
   that a session has, which runs an agent as `root` on a worker without agent
   isolation.
3. **The transport is the recorded route.** The command leaves through
   `cloud_runtime::ssh::Connection::pinned_command`, which uses `-F none`,
   `BatchMode=yes`, `IdentitiesOnly=yes`, `StrictHostKeyChecking=yes`,
   `GlobalKnownHostsFile=/dev/null` and the `known-hosts-<worker>` file of that
   cloud. An exec never records a host key, and it never deploys, resumes or
   repairs a cloud.
4. **One quoted word.** The command and the frame text reach the worker only as
   single-quoted words of Horizon's own shell text, with every embedded quote
   closed and reopened. Horizon refuses a NUL byte, an empty text and a text
   longer than 8192 bytes before it builds any text.
5. **No user switch.** A command whose first word of any simple command is `su`,
   `sudo`, `doas`, `setpriv`, `runuser`, `pkexec`, `nsenter` or `chroot` is
   refused with `cloud_exec_refused: user switch`, also when a path or an
   environment assignment precedes it. This refusal is an answer-quality check
   and not a security control: uid 10001 holds no capability that the launcher
   did not remove.
6. **The exit status is the command's own.** The wrapper ends with
   `exec /bin/sh -c <command>`, so the dropped shell becomes the command and
   `ssh`'s own exit code is the status that the kernel reported for it. Nothing
   of Horizon's follows the command in that shell, and Horizon reads no status
   from either stream: no process the command starts, and no later writer that
   still holds its stdout or stderr, can change the status. The launcher's own
   refusal codes 20, 21, 22 and the transport's 255 are read only when the
   account frame is absent, so a command that exits with one of those codes keeps
   its own status.
7. **The bound.** The command has a wall-clock bound of 1000 to 120000 ms. On
   expiry Horizon kills the process group of the SSH client, so no descendant
   survives. Output over 256 KiB on either stream is a failure, not a cut
   answer.
8. **A ready cloud.** The background thread reads the cloud's own saved record
   before the route, and refuses every stage other than `Ready` and a cloud
   without a record. A command never reaches a worker that is deploying, stopped
   or deleted.
9. **The audit keeps no text.** Horizon writes one private line per call with the
   actor, the cloud ID, the byte length and a SHA-256 digest of the command, the
   outcome, the exit status, the account and the duration. It never records the
   command text, because a command can carry a token that the caller pasted into
   it, and a journal that holds text turns one mistake into a stored secret. The
   digest compares two calls and cannot replay one.

## What Horizon does not guarantee

- **What the command does.** It has the whole authority of the agent account of
  that worker. It reads and writes every file that account owns under
  `/workspace`, including `/workspace/home`, `/workspace/sessions/*`,
  `/workspace/session-data/*` and the worktrees of other sessions. It pushes to
  every repository the worker's grant allows and merges pull requests through
  the brokered GitHub API. Horizon does not examine what the command asks GitHub
  to do; the worker-side broker and its grant decide that.
- **A token in no process.** The agent's `git` uses the loopback proxy and its
  `gh` uses the broker socket, so the agent user holds no token. That property
  comes from the image and its GitHub service, not from this tool. A worker
  without agent isolation is refused, so this tool cannot be used to read the
  root-only token chain, the request log or the authorized keys. It does not
  prove that the image has no other leak.
- **That the output is honest.** A caller can print text that looks like the
  account frame, and it reaches the answer as caller output; the account frame
  itself is the first line of the stream, printed before the caller's command
  starts, so no caller byte can precede it. The exit status is the command's own
  and no stream can change it, but it is not proof that a side effect happened,
  and the text of a `gh pr merge` in the output is not proof of a merge. Text a
  process leaves behind after the command exits still arrives in the answer,
  because the SSH client reads both streams until they close.
- **That the account is unique to the caller.** Every agent of a worker runs as
  one user. Two sessions of one worker see each other's files, as the file-drop
  boundary states too.
- **That the launcher has no side effect.** `horizon-worker-tailnet agent` runs
  its handoff before the drop: it creates and adopts the shared workspace paths
  under its upload lock, and it consumes `/workspace/siblings.json` when that
  file exists. The exec does not serialize with a deployment or with another
  exec of the same worker. Do not run a command beside a deployment that stages
  siblings.
- **A serialized call.** An interrupted answer leaves the command possibly
  running. There is no operation ID: a caller that repeats the tool call runs
  the command again, which matters for `git push` and `gh pr merge`.
- **The journal.** The append is best effort and a crash can lose its last line.
  The authoritative record of GitHub writes stays the worker-side
  `/run/horizon-github/requests.log`, which the agent user cannot read.

## Report

- A command that runs as any account other than `10001`.
- A path that runs the command when the isolation marker is absent, when the
  launcher is absent, or when the cloud is not `Ready`.
- A connection that accepts an unknown or a changed host key, or that enrolls a
  key during an exec.
- A command text that reaches a shell without its own quoting, an account frame
  that a caller's text can precede, or an exit status that a stream, a background
  process or a later writer can change.
- A timeout that leaves a descendant process, or output that arrives cut.
- A journal line that holds the command text, a path of the SSH identity so that
  it can be read, or a credential.
- File I/O, a provider call, a process start or a wait for a process on the UI
  thread.
- A cloud of another workspace, or a cloud that is ambiguous, that a caller can
  command.
