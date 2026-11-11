---
procedure: named-secrets
feature: Named secrets for agent panels
platforms: [linux, macos]
cost: none
destructive: yes
secrets: [named secret values of the test only]
owner: peters
---

# Named secrets test procedure

## 1. Purpose

This procedure proves that a named secret of a person becomes an environment
variable of every agent panel that Horizon starts on one computer, that a plain
terminal panel and a terminal on a machine get none, that a value is never drawn
again and never written to the settings or to a saved session, and that a second
computer that follows one server reads the same value.

## 2. Applicability

- Candidate: each candidate that changes the named secrets, the Settings window
  or the link to a server.
- Platforms: Linux with the local device smoke fixture. The macOS lane uses an
  isolated desktop session and the same steps.
- This procedure does not test: a real key of a provider, a real machine over
  SSH, or the value of another feature.

## 3. Safety

> **CAUTION:** USE A MARKER VALUE AND A NEW STATE DIRECTORY. Never type a real
> key of a provider into this procedure. The values of a fixture are in clear
> text in its private state, and that state must be a new directory that this run
> owns.

> **CAUTION:** THE FIRST TASK WRITES INTO THE OS CREDENTIAL STORE OF THIS
> COMPUTER WHEN IT RUNS WITHOUT THE FIXTURE. Prefer the fixture, which writes
> only its own file. If you run the tasks on this computer, delete each named
> secret at the end, and check the store for the name.

> **CAUTION:** CLOSE ONLY THE DEVICE PANEL AND THE FIXTURE OF THIS RUN. Do not
> stop another Horizon instance, another fixture, or the Horizon of the person.

## 4. Equipment and preconditions

- A Linux computer with Xvfb, Openbox, x11vnc, `bwrap`, `dbus-daemon` and
  Python 3, or a macOS test machine with an isolated desktop session.
- A frozen candidate binary, `<frozen>/horizon`, and its SHA-256 record.
- A private evidence directory, `<evidence>`, outside the checkout.
- A panel file, `<evidence>/panels.json`, that opens one agent panel and one
  terminal panel:

  ```json
  [{"name": "Secrets agent", "kind": "pi", "command": "/bin/sh",
    "args": ["-c", "printf 'agent token:%s\\n' \"$HORIZON_TEST_TOKEN\"; sleep 3600"],
    "position": [40, 560], "size": [560, 380]},
   {"name": "Secrets terminal", "kind": "shell", "command": "/bin/bash",
    "args": ["--noprofile", "--norc"], "position": [620, 560], "size": [560, 380]}]
  ```

- A marker value, `<marker>`, of the first run, such as `first-marker-7f3a`.
- A second marker value, `<marker2>`, such as `second-marker-2c81`.
- A second computer with one Horizon server, for section 6.8.

## 5. Setup

1. Start the fixture with one named secret and the panel file:

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <frozen>/horizon --native-view \
     --state <evidence>/fixture --panels <evidence>/panels.json \
     --named-secret HORIZON_TEST_TOKEN=<marker>
   ```

   Result: The manifest prints `vnc_address`. The Settings store of the fixture
   lists `HORIZON_TEST_TOKEN` in its `secrets` section, and the file
   `<evidence>/fixture/data/named-secrets.json` holds the value at mode `0600`.

2. Open the Device panel of the calling workspace on `vnc_address`, and check
   that `frame_sequence` advances.

   Result: The isolated desktop is visible and live.

## 6. Tasks

### 6.1 NS-1 — An agent panel reads the variable

1. Read the text of the panel `Secrets agent`.

   Result: The panel shows `agent token:<marker>`.

### 6.2 NS-2 — A terminal panel does not read it

1. Type `printf 'terminal token:%s\n' "$HORIZON_TEST_TOKEN"` in the panel
   `Secrets terminal`, and press Enter.

   Result: The panel shows `terminal token:` and no marker.

### 6.3 NS-3 — The Settings window lists the name and holds no value

1. Open the Settings window, and select the tab `Secrets`.

   Result: The row `HORIZON_TEST_TOKEN` shows `Stored on this computer`. The
   value `<marker>` is on no row.

2. Press `Replace value` on the row, type `<marker2>` in the field that appears,
   and press `Store`.

   Result: The field hides the text while you type it. The row shows `Stored on
   this computer` again, and no text of the value.

3. Save the settings.

   Result: The status line shows `Saved`.

### 6.4 NS-4 — A restored agent panel reads the new value

1. Quit the candidate with the window close button.

   Result: The fixture stops the candidate and keeps its private home, because
   this run used `--reuse-state`.

2. Start the fixture again, with the same state and no new named secret:

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <frozen>/horizon --native-view \
     --state <evidence>/fixture --reuse-state --resume-session
   ```

   Result: The board comes back with the panel `Secrets agent`.

3. Read the text of the restored panel `Secrets agent`.

   Result: The panel shows `agent token:<marker2>`. The value came from the
   credential store of the fixture, because the saved session holds no value.

### 6.5 NS-5 — A name whose value this computer does not hold

> **CAUTION:** THIS STEP DELETES THE FILE OF THE VALUES OF THE FIXTURE. It is the
> file of this run only.

1. Stop the fixture, delete `<evidence>/fixture/data/named-secrets.json`, and
   start it again with `--reuse-state --resume-session`.

   Result: The tab `Secrets` lists `HORIZON_TEST_TOKEN` and shows `no value on
   this computer`.

2. Read the text of the restored panel `Secrets agent`.

   Result: The panel shows `agent token:` and no marker, because this computer
   holds no value for the name.

### 6.6 NS-6 — A reserved name is refused

1. Type `HOME` in the name field of the `Secrets` tab, press `Add`, and read the
   line under the field.

   Result: The line says that the name belongs to Horizon or to the system, and
   no row appears.

### 6.7 NS-7 — Delete, and the saved session holds no value

1. Press `Delete` on the row `HORIZON_TEST_TOKEN`.

   Result: A line asks for the confirmation, and names the panel that keeps a
   value that it already started with.

2. Press `Delete` in that line.

   Result: The row goes.

3. Search the private home of the fixture for the marker values:

   ```sh
   grep -rl '<marker>' <evidence>/fixture/data/home
   grep -rl '<marker2>' <evidence>/fixture/data/home
   ```

   Result: No file of the home of the fixture holds a marker. A saved session, a
   panel title and a log line hold none.

### 6.8 NS-8 — Two computers and one server

1. Start one Horizon server, and start two fixtures that name it with
   `--server-target`.

   Result: Each fixture shows `Server: in step` in the sidebar.

2. Add the named secret `HORIZON_TEST_TOKEN` with the value `<marker>` in the
   `Secrets` tab of the first computer, and wait for the link.

   Result: The tab of the second computer lists `HORIZON_TEST_TOKEN`, and an
   agent panel that the second computer starts shows `agent token:<marker>`.

3. Delete the named secret in the `Secrets` tab of the second computer.

   Result: The tab of the first computer loses the row, and an agent panel that
   the first computer starts reads no value.

4. Search the home of the server for the marker:

   ```sh
   grep -rl '<marker>' <server home>
   ```

   Result: No file holds the marker in clear text.

## 7. Pass criteria

- Every result in section 6 appears.
- An agent panel reads each named secret of this computer, and a terminal panel
  reads none.
- No value appears in a drawn row, in the settings store, in a saved session or
  in the server home.

## 8. Cleanup

1. Close the Device panel of this run.

   Result: The viewer closes, and the fixture keeps running.

2. Stop the fixture with Ctrl-C in its terminal.

   Result: The fixture ends, and it removes the private state of a run that did
   not use `--reuse-state`.

3. Remove the state and the evidence directories of this run.

   Result: No process and no file of this procedure remains.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
