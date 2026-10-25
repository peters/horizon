---
procedure: device-smoke-extra-panels
feature: Extra workspace panels in the local device smoke fixture
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Device smoke extra panels test procedure

## 1. Purpose

This procedure makes sure that the fixture opens a panel that a procedure needs,
such as an agent panel that runs a scripted client, beside the default panels
and inside the native Device panel the person watches. It also makes sure that
an unusable panel file stops the fixture before Horizon starts.

## 2. Applicability

- Candidate: each candidate that changes `scripts/device-smoke/serve.py` or the
  panel file it reads.
- Platforms: Linux with the local device smoke fixture.
- This procedure does not test: a real agent that signs in to a model provider,
  any cloud operation, or the tools of a panel.

## 3. Safety

> **CAUTION:** USE ONLY A NEW STATE DIRECTORY FOR EACH RUN. The fixture removes
> and creates its private state, and it must never touch the state of the person
> who runs the test.

> **CAUTION:** CLOSE ONLY THE DEVICE PANEL AND THE FIXTURE OF THIS RUN. Do not
> stop another Horizon instance or another fixture.

## 4. Equipment and preconditions

- A Linux computer with Xvfb, Openbox, x11vnc, `bwrap`, `dbus-daemon` and
  Python 3.
- A frozen candidate binary and its SHA-256 record.
- A private evidence directory, `<evidence>`, outside the checkout.
- A panel file, `<evidence>/panels.json`, for example:

  ```json
  [{"name": "Scripted client", "kind": "codex", "command": "/usr/bin/python3",
    "args": ["-c", "print('panel of the procedure')"], "position": [40, 560], "size": [560, 380]}]
  ```

## 5. Setup

1. Start the fixture with the panel file:

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <frozen>/horizon --native-view \
     --state <evidence>/fixture --panels <evidence>/panels.json
   ```

   Result: The printed manifest shows `vnc_address` and `display`. The command
   line of the candidate in `lab.json` is unchanged.

2. Create the native Device panel for `vnc_address` in your own workspace, then
   inspect it three times, two seconds apart.

   Result: `connection` is `connected`, `image_received` and `image_displayed`
   are true, and `frame_sequence` advances.

## 6. Tasks

### 6.1 P1 — The panel is on the board

1. Look at the native Device panel.

   Result: The board shows the three panels: the heartbeat, the input terminal
   and the panel of the file.

2. Find the application child in the fixture process tree and compare it with
   the frozen binary.

   Result: `/proc/<pid>/exe` is the frozen binary and its SHA-256 equals the
   recorded value.

### 6.2 P2 — A panel without a position is placed out of the way

1. Remove `position` and `size` from the entry, then start a second fixture with
   a new state directory.

   Result: The panel starts in a column to the right of every panel the fixture
   places and does not cover them.

2. Start a third fixture with `--firefox-system-access` and the same entry
   without a position.

   Result: The panel starts to the right of the browser panel of that board and
   does not cover it.

### 6.3 P3 — An unusable file stops the start

1. Start the fixture with a panel file that has no string `name`, one whose
   `kind` is not a panel kind (for example `agent`, `Codex` or `3`), one whose
   `args` holds a number, one whose `position` is a string, and one that is not
   a list.

   Result: Each run stops before Horizon starts, and each message names the file
   and the entry.

### 6.4 P4 — No file keeps the default board

1. Start the fixture without `--panels`.

   Result: The board shows only the two default panels.

## 7. Pass criteria

- P1 shows the extra panel on the board and the frozen binary in use.
- P2 places a panel without a position out of the way of the panels the fixture
  places, on the default board and on the Firefox disclosure board.
- P3 stops every unusable file before Horizon starts.
- P4 leaves the default board unchanged.
- No step changes the settings, the sessions or the running Horizon of the
  person who runs the test.

## 8. Cleanup

1. Close only the Device panels that this run created.

   Result: The panels are absent from your workspace.

2. Stop the fixture with its own stop path.

   Result: The application child, the display and the VNC server stop.

3. Remove the frozen binaries and `<evidence>`.

   Result: No file and no process of this run stays on the computer.

## 9. Record of results

Put the results in the pull request. Write a report in
`docs/testing/reports/` when the run must be kept.
