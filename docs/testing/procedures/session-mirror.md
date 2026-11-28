---
procedure: session-mirror
feature: The session mirror
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Session mirror test procedure

## 1. Purpose

This procedure proves that one computer publishes its agent sessions to a server,
and that a second computer of that server shows the list and one session, and
follows the new entries of that session. It also proves that the viewer cannot
change a mirrored session, and that it stores nothing.

## 2. Applicability

- Candidate: a build of this repository with `--mirror-sessions`.
- Platforms: Linux for the publisher and for the two Horizon windows. The server
  runs on either platform.
- This procedure does not test: the store of Antigravity, which this build cannot
  read; a message from a viewer to a session, which this slice does not build.

## 3. Safety

> **CAUTION:** THE PUBLISHER SHOWS THE TEXT OF THE SESSIONS OF THIS COMPUTER TO EVERY COMPUTER OF THE SERVER. Run this procedure with the stand-in home of `scripts/device-smoke/session_store_standin.py`, never with the home of a person.

> **CAUTION:** STOP THE PUBLISHER AT THE END OF THIS PROCEDURE. A publisher that runs keeps sending the list of its sessions.

## 4. Equipment and preconditions

- A build of `horizon` of the candidate.
- Two computers of one Horizon server, or one computer with two Horizon windows
  of two different homes and one server in a private home.
- Python 3 for the stand-in home.

## 5. Setup

1. Write a stand-in home of four sessions, and keep it serving:

   ```sh
   python3 scripts/device-smoke/session_store_standin.py --home /tmp/mirror-home --serve
   ```

   Result: three lines name the home, the live process and the four sessions.
   The command stays in the foreground.

2. Start the server in a private home, with a listener on `127.0.0.1:7000`:

   ```sh
   HORIZON_SERVER_HOME=/tmp/mirror-server horizon --server --listen 127.0.0.1:7000 --name 127.0.0.1
   ```

   Result: the log names the socket and the listener.

3. Ask the server for one invitation:

   ```sh
   HORIZON_SERVER_HOME=/tmp/mirror-server horizon --server-invite
   ```

   Result: the command prints a link, a code and the minutes that are left.

## 6. Tasks

### 6.1 M1 — the publisher joins and starts

1. Join the publisher against the stand-in home:

   ```sh
   HOME=/tmp/mirror-home HORIZON_MIRROR_HOME=/tmp/mirror-publisher \
     horizon --mirror-sessions --join '<the link>'
   ```

   Result: one line reads `horizon mirror: this computer joined the server at
   127.0.0.1:7000 as <name>-sessions.`, and the command ends with code 0.

2. Start the publisher:

   ```sh
   HOME=/tmp/mirror-home HORIZON_MIRROR_HOME=/tmp/mirror-publisher horizon --mirror-sessions
   ```

   Result: one line names the address and the device name, one line reads
   `horizon mirror: connected to the server.`, and one line reads `horizon mirror:
   the list holds 4 sessions: claude 2, codex 1, pi 1, antigravity 0.`

3. Read the home of the publisher.

   Result: `/tmp/mirror-publisher` is at mode 0700, and the files in it are at
   mode 0600. `/tmp/mirror-home` holds the same files as before the first start,
   and no file of `/tmp/mirror-home/.horizon`.

### 6.2 M2 — a second computer shows the list

1. Start a Horizon window of the second computer, or of the second home, and join
   it to the same server through **Settings** > **Server**, as the procedure
   `horizon-server` gives.

   Result: the sidebar reads `Server: in step`.

2. Read the group `COMPUTERS` of the sidebar.

   Result: under the name of the publishing computer, four rows hold the two
   letters of their program, a title and a time of the last change. The live
   session shows a dot in the accent colour and no time. One line reads
   `Antigravity is not mirrored`.

### 6.3 M3 — a press shows one session

1. Press the row of the live Claude session.

   Result: the dock shows the title of the session and, under it, `Claude Code on
   <computer> · read only`. The chip reads `live`. The rows of the conversation
   hold the text of the person, the text of the agent and the row of the tool
   `Read` with its request. No result of the tool is there.

2. Read the place of the field.

   Result: one centred line reads `This session runs on <computer>. You read it
   here.` There is no field, no chip and no send control.

3. Type in the window, and press no control of the mirrored session.

   Result: nothing of the typing reaches the session.

### 6.4 M4 — a new entry arrives

1. Add one text of the agent to the live session of the stand-in:

   ```sh
   python3 scripts/device-smoke/session_store_standin.py --home /tmp/mirror-home \
     --append "The retry is in the upload now."
   ```

   Result: one line reads that the text is in the session.

2. Read the dock of the session within five seconds.

   Result: one more row of the agent holds the new text, under the rows that were
   there.

### 6.5 M5 — the follow ends

1. Press the row of the front desk in the sidebar, and then the row of the live
   session again.

   Result: the dock shows the conversation that the press named, and then the
   session again with its rows.

2. Stop the publisher with Ctrl+C.

   Result: the process prints `horizon mirror: stopped.` and ends.

3. Read the group `COMPUTERS` of the viewer.

   Result: the rows of that computer leave the sidebar within half a minute.

## 7. Pass criteria

- The publisher prints one line at its start, one for its connect and one for the
  list, and no text of a session.
- A start or a join writes no file outside the home of the publisher.
- The viewer shows four rows, and the live row shows the dot.
- The dock shows the header and the line of a read-only session, and no field.
- A new entry of the session arrives in the dock within five seconds of the
  append.
- A stopped publisher leaves no row in the viewer.

## 8. Cleanup

1. Stop the publisher with Ctrl+C.

   Result: the process ends.

2. Stop the stand-in with Ctrl+C.

   Result: it prints `session-store-standin: stopped`, and its live process is
   gone.

3. Stop the server, and remove the three temporary folders:

   ```sh
   rm -rf /tmp/mirror-home /tmp/mirror-publisher /tmp/mirror-server
   ```

   Result: no process of this procedure runs.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
