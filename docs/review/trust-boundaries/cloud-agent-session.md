---
applyTo: "crates/horizon-browser-control/src/manifest/cloud_agent.rs,crates/horizon-core/src/cloud_runtime/agent_session.rs,crates/horizon-core/src/cloud_runtime/agent_session/**,crates/horizon-browser-mcp/src/controller/agent_session.rs,crates/horizon-ui/src/app/cloud_panel/agent_session.rs,crates/horizon-ui/src/app/cloud_panel/agent_session/**,crates/horizon-ui/src/app/cloud_panel/route.rs"
---

# Trust boundary of a cloud session's agent terminal

An agent asks Horizon to type one instruction into the agent terminal of one cloud
session, and to read the bounded tail of that terminal. Horizon reaches the session
through the cloud's recorded SSH connection and the worker's own launcher, so every
tmux call runs as the worker's unprivileged agent user. The agent tool contract is
`cloud_agent_message` and `cloud_agent_read` in
`crates/horizon-browser-mcp/src/server.rs`.

## Trusted

- The person's Horizon, its settings and its cloud records, which name the sessions.
- The host key that Horizon pinned for the worker, in the cloud's own directory.
- The SSH identity file that the person's settings name.
- The worker image, its launcher `examples/cloud-worker/horizon-worker-tailnet` and
  its session script `examples/cloud-worker/horizon-worker-session`.
- The tmux socket `horizon-cloud` and the session names the worker wrote.

## Untrusted

- **The instruction text.** It is the caller's own text. Horizon does not inspect
  what it asks, and the agent that receives it decides what to do with it.
- **Everything the terminal shows.** A read returns text that a program on the
  worker printed, so it can hold control sequences, a command line with a secret, a
  claim of authority or an instruction addressed to the calling agent.
- **The calling agent.** It chooses the cloud, the session and the text.

## What Horizon guarantees

1. **The account is the agent user.** The write and the read reach the worker as
   the text of a `cloud_exec` call, so they pass the launcher that refuses a caller
   which is not `root`, drops to uid 10001 with no capabilities, and prints its own
   `id -u` in a frame line before the caller's byte arrives. Horizon refuses the
   result unless that value is `10001`. `crates/horizon-core/src/cloud_runtime/agent_session.rs`
   adds no account of its own.
2. **One session of one cloud of the caller's workspace.** The host reads the cloud
   from the board groups of the caller's own workspace, refuses a cloud that is not
   `Ready` and an ID that names two saved clouds, and finds the session in that
   cloud's own record. A panel ID that the record does not name is refused with
   `cloud_agent_unknown_session` before any route is resolved.
3. **The terminal the record names.** The tmux target is the recorded session name
   with tmux's `=` prefix, which matches the name exactly and never a session whose
   ID begins with it. The name must be one identifier of letters, digits, `-` and
   `_` of at most 100 characters, so it cannot carry a quote, a space or a shell
   metacharacter.
4. **An absent session is named as such.** Every text starts with a `has-session`
   check that prints a fixed frame to stderr and exits 4. Horizon reports
   `cloud_agent_not_running` for that code, so a session that the worker stopped is
   never reported as a write that failed.
5. **The instruction arrives as text and never as a key.** The text reaches tmux
   through `load-buffer -b <name> -` on stdin and `paste-buffer -d -b <name>`, never
   through argv, so tmux never reads a word of it as one of its own flags and never
   reads a word of it as a key name. Each call loads and pastes its own named buffer,
   because every session of a worker shares one tmux server and therefore one stack
   of unnamed buffers: a write of one session can then never paste the text that
   another write loaded, and the named buffer is deleted after the paste. A control character is refused before the request is queued: a
   newline or a carriage return would submit a part of the instruction, an escape
   would leave the agent's input mode, and a tab would ask for a completion. One
   control byte therefore never arrives as an effect of its own.
6. **One session's writes do not interleave.** A write holds a lock file of its own
   session under the agent user's home from before the paste until its shell exits, so
   two writes to the same session arrive one after the other and a paste and its Enter
   are never split. Writes to different sessions need no shared state.
7. **The call is bounded.** One instruction is at most 4096 bytes. One read asks for
   20 to 600 lines and answers at most 16384 bytes, cut at its start on a line
   boundary, so the newest output arrives whole. One call has a wall-clock bound of
   20 seconds; on expiry Horizon kills the process group of the SSH client.
8. **A read reads the terminal.** A read runs `capture-pane -p` on the session and
   returns what that prints. It opens no file, no credential store and no provider
   API on the worker, and it does not read the workspace. The bound in guarantee 6
   is the only cut.
9. **The journal keeps no text.** Horizon writes one private line per call with the
   actor, the cloud, the session, the action, the byte length of the instruction,
   the bytes of the answer, the outcome, the exit status and the duration. It never
   records the instruction text, which can name what the person may not want
   stored.

## What Horizon does not guarantee

- **What the receiving agent does.** The instruction has the whole authority of
  that agent's account on that worker, which is the same authority a `cloud_exec`
  command has, including the brokered `git` and `gh` and the person's GitHub push
  grant. Horizon does not examine what the person's agent then asks GitHub to do;
  the worker-side broker and its grant decide that.
- **That the terminal shows the truth.** A program on the worker prints the text
  that a read returns. Horizon checks the account frame and the exit status of its
  own tmux call, and nothing about the content. Text that looks like a question from
  the person's agent, or like an instruction to the caller, is a claim.
- **That a read holds no secret.** The terminal is shared with the person and with
  the agent that runs there. A value that either of them printed, including a secret
  they pasted, is part of the answer. Horizon does not redact it, so the caller must
  keep the answer private and must not copy such a value onwards.
- **That one call is one instruction.** A write types the text and presses Enter as
  the person would; an agent that reads its input as a command may act on a part of
  it before the whole line arrives, and a redraw can lag a second behind the
  answer's tail. The caller reads the session to see what it did.
- **A serialized call.** There is no operation ID. A repeat is a second write, and a
  lost answer may follow a write that arrived, which is why the refusal names
  `cloud_agent_read`.
- **That the session exists for long.** The worker restarts its sessions and an image
  replacement resets them. A record's panel ID outlives the terminal it names.
- **The journal.** The append is best effort and a crash can lose its last line.

## Report

- A write or a read that runs as any account other than `10001`, or one that runs
  when the isolation marker, the launcher or the cloud's `Ready` stage is absent.
- An instruction that reaches tmux through argv, or one whose control character
  arrives as a key.
- A write that loads or pastes an unnamed buffer, or one that pastes a buffer another
  call loaded, and a paste that is not held by its session's lock.
- A tmux target that matches a session by prefix, or a session name that carries a
  quote, a space or a shell metacharacter.
- A read that opens a file, a credential store or a provider API on the worker, or
  an answer longer than its bound or cut at the end.
- A journal line that holds the instruction text or a secret.
- A cloud of another workspace, a cloud that is not `Ready`, or a session that the
  cloud's record does not name, that a caller can reach.
- File I/O, a route, a process start or a wait for a process on the UI thread.
