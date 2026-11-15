---
applyTo: "crates/horizon-core/src/session_pickup.rs,crates/horizon-core/src/session_pickup/**,crates/horizon-core/src/runtime_state/agent_sessions.rs,crates/horizon-core/src/runtime_state/agent_sessions/codex.rs,crates/horizon-core/src/runtime_state/claude_live_sessions.rs,crates/horizon-ui/src/app/session_pickup.rs,crates/horizon-ui/src/app/session_pickup/**,crates/horizon-ui/src/app/welcome.rs,crates/horizon-ui/src/app/welcome/pickup.rs"
---

# Agent session pickup boundary

Read `docs/first-steps.md` for the page that shows this list.

This boundary covers the section **Pick up where you left off** of the first
start page, the window **Recent agent sessions** of the titlebar Menu, and the
rows that both show.

## What is untrusted

Every file of a session store of an agent program is untrusted. Another program
wrote it. A repository that a person cloned can also have made an agent write
anything into a title or into a first message. This covers, for each row:

- the session id,
- the folder (`cwd`),
- the title,
- the first message of the person,
- the time of the last activity.

The name of a file in a store, its size and its access time are untrusted too.

## What the code guarantees

1. **Nothing of a session file is executed or interpreted.** Two values reach a
   command line: the session id and the folder.
   - A session id reaches the command line only after a check of its form. The
     form is the one that the program uses: a UUID for the two programs of this
     round. A row whose id is not of that form is not shown and logs one line.
   - The folder reaches the process only as its working directory, never as a
     shell string. No shell stands between Horizon and the agent program: the
     program and its arguments are one argument vector.
   - A folder that is not an existing directory ends with the row state that
     says it is gone, and the row cannot open.
2. **A title or a message is only text.** Horizon shows it on one line, with
   the control characters removed, with at most `MAX_TITLE_CHARS` characters,
   and every run of white space made one space. Horizon draws it and never
   passes it to another program.
3. **The read is bounded.**
   - At most `MAX_SESSION_ROWS` rows are shown, the newest first.
   - At most `MAX_SESSION_AGE_DAYS` days of a session are read; an older session
     is not a row.
   - The Claude Code store is read to a maximum of 64 session files, and one
     file is read on its head and on its bounded tail.
   - The Codex store answers one bounded query, and the title column is cut to
     256 characters by that query.
   - The live registry of Claude Code is read to a maximum of 4,096 entries of
     at most 64 KiB each.
4. **No read, stat or parse runs on the UI thread.** `horizon_core::session_pickup`
   starts every read with `ui_thread::blocking`, and the window runs it on one
   worker thread with a result channel. A test of the window runs the whole
   suite in the strict mode of the guard through a frame that shows the section.
5. **Nothing is written and nothing is deleted.** This function opens files for
   reading only. It writes no file of a session store, no settings file and no
   session file of Horizon.
6. **A session of an open panel of this Horizon is not offered again.** The
   caller gives the set of `(program, session id)` of the board, and a row in
   that set is left out.
7. **A session that another process has open now is marked.** Two programs in
   one conversation is a known hazard. The row says `open in another program`
   and opens only after a second confirmation of the person. For Claude Code the
   live registry `~/.claude/sessions/<pid>.json` answers it, through
   `horizon_core::live_claude_session_ids`, which also ignores an entry of a
   process that has ended. Codex keeps no such registry today, so no Codex row
   carries this mark.

## What the code does not guarantee

- It does not guarantee that a session still resumes. The agent program decides
  that. `--resume` of a removed conversation fails in the panel, and Horizon
  shows the failure of that panel as it shows every other one.
- It does not guarantee that a folder is the repository that it was. The rows
  show the text of the store. A folder that another program replaced is a
  different folder with the same name.
- It does not guarantee that a title says what the conversation is about. The
  agent program, or a repository that an agent worked in, could have written it.
- It does not lock out the agent program of the session during the open. The
  mark of guarantee 7 is read at the time of the row, and the program can start
  or stop the session in the time between the row and the press of the person.

## Review notes

- Do not add a second reader of a store that `horizon-core` already reads. The
  pickup builds on `AgentSessionCatalog` and its provider readers.
- A new program is a new row kind only after the boundary names the form of its
  session id, its store, and its live registry or the absence of one.
- Keep a session id out of a shell string, a log of one line of the row, and a
  window title.
- A bound that the code states must be applied to the bytes that are actually
  read, not to a size that was measured before the read.
