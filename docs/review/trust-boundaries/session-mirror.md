---
applyTo: "crates/horizon-core/src/session_mirror.rs,crates/horizon-core/src/session_mirror/**,crates/horizon-core/src/runtime_state/agent_sessions.rs,crates/horizon-core/src/runtime_state/claude_live_sessions.rs,crates/horizon-sync/src/live/mirror.rs,crates/horizon-sync/src/supervisor/live_mirror.rs,crates/horizon-sync/src/live/hub.rs,crates/horizon-sync/src/live/protocol.rs,crates/horizon-sync/tests/mirror.rs,crates/horizon-ui/src/mirror_sessions.rs,crates/horizon-ui/src/mirror_sessions/**,crates/horizon-ui/src/app/session_mirror.rs,crates/horizon-ui/src/app/session_mirror/**,crates/horizon-ui/src/app/sidebar/mirror_rows.rs,crates/horizon-ui/src/app/orchestrator_dock/mirror.rs,scripts/device-smoke/session_store_standin.py,docs/architecture/session-mirror.md"
---

# Trust boundary of the session mirror

The session mirror shows the agent sessions of one computer on the other
computers of the same Horizon server, in the conversation dock. The computer
that has the sessions runs the publisher, `horizon --mirror-sessions`. The
server relays the frames. A viewer is a Horizon window of another computer of
that server.

The design is in
[`docs/architecture/session-mirror.md`](../../architecture/session-mirror.md).
The publisher reads the session stores of the agent programs, so the boundary of
those stores is the one of
[`agent-session-pickup.md`](agent-session-pickup.md). The frames travel on the
live link, whose boundary is
[`shared-terminals.md`](shared-terminals.md): the greeting, the login and the
bounds of a frame are the same.

**The owner of the product decided this boundary.** Every computer that is
linked to a server can read the sessions that a publisher of that server sends.
A person who controls one linked computer, or the server, can therefore read
those sessions while the publisher runs. The server is trusted with the content
that goes through it. The link has the confidentiality of its transport only.

**The person starts the publisher.** The publisher is a process of the computer
that has the sessions. No setting of a viewer, and no frame of another computer,
starts it. The person ends it with the end of that process.

## Untrusted: every value of a session store

- **A store is text that another program wrote.** A line of a transcript, a
  title, a tool name and a tool request all come from a program that the mirror
  does not control. Nothing of them reaches a command line, a path, a log line
  or the clipboard of this computer.
- **The reader only reads.** `crates/horizon-core/src/session_mirror.rs` opens no
  socket, writes no file and starts no process. It opens a session file only
  when the path is a regular file, and it never opens a file of a store that the
  session catalog did not name.
- **Each read has a bound.** The reader takes at most 1 MiB from the end of one
  session file, at most 50 sessions of one list, and at most 60 entries of one
  session. A file above the bound reads its tail and says so with one notice.
- **Each text has a bound and no control character.** A text of the person or of
  the agent holds at most 4 000 characters, the first line of a tool request at
  most 200, a title at most 160. The reader uses the rule of the session pickup,
  so no control character moves the cursor of a terminal or hides the end of a
  line.
- **The mirror sends no result of a tool and no thinking text.** A result of a
  tool is where a file, a key or the output of a command stands. An entry is one
  of four things: the text of the person, the text of the agent, the name of a
  tool with the first line of its request, and one notice of Horizon.
- **The frame of one session holds at most 60 entries and 192 KiB.** The server
  cuts each text again at its bound before it relays a frame, and the ceiling of
  one frame of the link is 256 KiB.
- **A program that this build cannot read is named with no session.** The reader
  says so, and it never guesses a shape that it did not measure.

## Untrusted: every frame of the mirror

- **A computer joins the mirror with `mirror_open`.** The server sends a frame of
  the mirror only to a link that sent that request. A computer of an older build
  reads no such frame, so its live link and its shared terminals are unchanged.
- **A computer sends `mirror_open` only to a server of version 10 or higher.** An
  older server reads no frame of the mirror and would end the link.
- **The server names the sender.** The server writes the `owner` of the entries
  of a session from the link that sent them. A frame cannot name another sender.
- **A follow has conditions.** The owner must be another computer that sent
  `mirror_open` and whose list holds that session. If not, the computer that
  asked reads `not_found`. At a limit, it reads `limit`.
- **The server stores nothing of a session.** The hub opens no file. A frame
  that waits for a link waits in memory, and a log line of the hub names a
  device and a count. The store and the log of the server hold no text of a
  session.
- **A session is relayed only to the computers of its own server.** The hub of a
  server holds the links of that server alone.

## What the viewer guarantees

- **Read only.** The mirror sends no key and no text to a session. A viewer
  cannot change, stop or answer a mirrored session. The only frames of a viewer
  are the follow, the end of a follow and `mirror_open`.
- **Every text is drawn as text.** No link, path or mark in a mirrored row takes
  a press. The marks of Markdown change only how a span is drawn.
- **The viewer stores nothing.** No entry and no list of a mirrored session
  reaches a file of the viewer. A restart shows the list again when a publisher
  sends it.
- **The frames are read on the thread of the live link.** A frame of the window
  reads a snapshot of what the pump read.

## Not guaranteed

- **A text can hold a secret.** The text of the person and of the agent can hold
  something that someone typed or that an agent quoted. The mirror does not find
  or remove it. A person who must not show a session to another computer of the
  server does not run the publisher on that computer.
- **The publisher is not authenticated by its stores.** It reads whatever the
  session stores of its home hold, as the session pickup does.
- **A viewer cannot prove that the list is complete.** A publisher that is not
  joined, or whose server is away, shows nothing, and the viewer shows no row
  for it.
