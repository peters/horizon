# The session mirror

A person has agent sessions that run on one computer: in a terminal, in an older
Horizon, or in an editor. The session mirror shows those sessions on the other
computers of the same Horizon server, in the conversation dock. The person reads
there what each agent does. The computer that runs the sessions changes nothing
in how it works.

This page gives what the mirror reads, how it travels, what the other computer
shows, and the trust boundary. The code of the reader is
`crates/horizon-core/src/session_mirror/`. The trust boundary of the stores that
it reads is the one of
[`agent-session-pickup`](../review/trust-boundaries/agent-session-pickup.md).

## The three parts

- **The publisher** runs on the computer that has the sessions. It is a mode of
  the `horizon` binary: `horizon --mirror-sessions`. It opens no window, reads no
  setting of Horizon and installs nothing. It reads the session stores of the
  agent programs and sends what it reads to the server.
- **The server** relays. It keeps nothing of a session on disk.
- **The viewer** is a Horizon window on another computer of the same server. It
  shows the sessions of each computer in the sidebar group `COMPUTERS`, and one
  session in the dock.

## What the publisher reads

The publisher only reads. It writes no file of an agent program.

| Program | The list of sessions | The entries of one session |
|---|---|---|
| Claude Code | `~/.claude/sessions/<pid>.json` for the sessions that run, and the catalog that the session pickup reads | `~/.claude/projects/<folder>/<id>.jsonl` |
| Codex | the catalog that the session pickup reads | `~/.codex/sessions/<year>/<month>/<day>/<file>.jsonl` |
| Pi | `~/.pi/agent/sessions/<folder>/<file>.jsonl` | the same file |
| Antigravity (`agy`) | the store under `~/.gemini`, where the run that builds this finds one that it can read | the same store |

A program whose store the publisher cannot read is named in the list with no
session, so the person sees that the program is there and not mirrored.

One session in the list holds: the program, the id of the session, the name and
the parent of its folder, a title of one line, the time of its last change, and
whether a process of that program runs it now.

One entry of a session is one of three kinds, as the dock draws them:

- the text of the person;
- the text of the agent;
- the name of a tool and the first line of what it was asked to do.

The publisher sends **no result of a tool**. A result is where a file, a key or
the output of a command stands, and the mirror exists to follow the work, not to
copy what the work read.

## Bounds

| What | Bound |
|---|---|
| Sessions in one list | 50, the newest first |
| Age of a session in the list | 14 days, or any age while a process runs it |
| Entries of one session that the publisher sends | the last 60 |
| One text of the person or of the agent | 4,000 characters |
| The first line of a tool request | 200 characters |
| Bytes read from the end of one session file | 1 MiB |
| How often the publisher reads the stores | once in 3 seconds, and only the files whose size or time changed |
| One frame of the mirror | 256 KiB |

A value outside a bound is cut. A file that does not parse gives the entries that
did parse, and one entry of Horizon that says the rest could not be read.

## How it travels

- The publisher joins the server one time with an invitation, as a computer
  does: `horizon --mirror-sessions --join <link>`. It keeps the token of that
  join in its own home, `HORIZON_MIRROR_HOME` or `~/.horizon-mirror`, at mode
  0700 with files at 0600. It reads and writes no `~/.horizon`.
- It opens the live link of the server, the link that the shared terminals use.
- It sends the list when the list changes.
- It sends the entries of a session only while a viewer follows that session. A
  viewer that opens a session in its dock sends a follow, and a viewer that
  leaves it sends an end of the follow. With no follower the publisher sends no
  entry of any session.
- The server relays the frames to the other computers of that server and keeps
  none of them.

## What the viewer shows

- The group `COMPUTERS` of the sidebar shows, under the name of a computer that
  publishes, one row for each session: a mark of two letters for the program
  (`CC`, `CX`, `PI`, `AG`), the title, and at the right the time of the last
  change, or a dot in `ACCENT` while a process runs the session. It shows at
  most 8 rows and then `and <N> more`, which opens the rest.
- A press on a row shows that session in the dock. The header reads the title,
  and under it `<program> on <computer> · read only`. The list draws the entries
  with the rows of a conversation. In place of the field the dock reads
  `This session runs on <computer>. You read it here.`
- The dock follows the session while it shows it: a new entry on the other
  computer arrives within a few seconds.
- The viewer stores nothing of a mirrored session. A restart shows the list
  again when the publisher sends it.

## Trust boundary

- **What is untrusted.** Every value of a session store is text that another
  program wrote. The publisher bounds each read and each text. The viewer draws
  each text as text: no link, no path and no mark in it takes a press, and the
  marks of Markdown change only how a span is drawn.
- **Who receives it.** Only the computers of the same server, which already
  hold the keys and the values of that server and can type in the shared
  terminals of each other. The mirror gives them nothing that a shared terminal
  of the same session would not show, and less: no result of a tool.
- **The person turns it on.** The publisher is a process that the person starts
  on the computer that has the sessions. No setting of a viewer and no request
  of another computer starts it. The person stops it with the end of that
  process.
- **The server keeps nothing.** The frames of the mirror are relayed and not
  written to the store or to the log of the server.
- **Read only.** The mirror sends no key and no text to a session. A viewer
  cannot change, stop or answer a mirrored session.
- **Not guaranteed.** The text of the person and of the agent can hold a secret
  that someone typed or that an agent quoted. The mirror does not find or remove
  it. A person who must not show a session to another computer of the server
  does not run the publisher on that computer.

## Not built

- A message from a viewer to a mirrored session.
- The screen of the terminal of a session. The shared terminals of two Horizon
  windows of the newer kind give that.
