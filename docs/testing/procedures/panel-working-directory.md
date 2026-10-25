---
procedure: panel-working-directory
feature: Panel launch in a workspace directory
platforms: [linux]
cost: none
destructive: yes
secrets: none
owner: peters
---

# Panel working directory test procedure

## 1. Purpose

This procedure proves that a panel starts in the directory of its workspace, and
that a panel never starts in the directory of the Horizon process when the
workspace directory is not available. It also proves that Horizon says why the
panel cannot start and asks for a directory instead.

## 2. Applicability

- Candidate: a Horizon build from this branch, built with
  `cargo build -p horizon-ui --bin horizon`.
- Platforms: Linux. The procedure uses the local device smoke fixture.
- This procedure does not test: cloud panels, SSH panels, and restored sessions
  of a saved session file. Those panels start a remote shell, where a local
  directory decides nothing.

## 3. Safety

> **CAUTION:** DO NOT RENAME A DIRECTORY THAT A REAL WORKSPACE USES. Do this
> procedure only in the private state directory of the fixture, and only while
> its fixture runs. The fixture removes that directory when it stops.

## 4. Equipment and preconditions

- A Linux computer with the tools in
  [the local device smoke guide](../../../scripts/device-smoke/README.md):
  Xvfb, Openbox, x11vnc, bubblewrap, dbus-daemon, Python 3 and `xdotool`.
- A Horizon host that can open a native Device panel. The person who runs the
  procedure watches that panel.
- One frozen candidate binary and its SHA-256 hash.

## 5. Setup

1. Build the candidate in a worktree that has its own target directory, and
   freeze the binary. Keep the hash for the report.

   ```sh
   export CARGO_TARGET_DIR="$PWD/target/device-smoke"
   cargo build -p horizon-ui --bin horizon
   smoke_bin=$(mktemp -d /tmp/horizon-smoke-bin.XXXXXX)
   cp "$CARGO_TARGET_DIR/debug/horizon" "$smoke_bin/horizon"
   sha256sum "$smoke_bin/horizon"
   ```

   Result: The build ends and prints the hash.

2. Write a preset file with one agent panel that prints its directory. The
   fixture needs no login, credential or agent binary for this panel.

   ```sh
   cat > /tmp/panel-cwd-presets.json <<'JSON'
   [{"name": "Pi cwd demo", "kind": "pi", "command": "/bin/sh",
     "args": ["-c", "printf 'working directory: '; pwd; exec sleep 900"]}]
   JSON
   ```

3. Start the fixture with a new state directory, and keep the command running.

   ```sh
   python3 scripts/device-smoke/serve.py --horizon "$smoke_bin/horizon" \
     --native-view --state /tmp/panel-cwd-1 \
     --presets /tmp/panel-cwd-presets.json
   ```

   Result: The fixture prints one JSON manifest with `display`,
   `vnc_address` and `pids`.

4. Read `lab.json` in the state directory and note the `vnc_address`. Create a
   **Device panel** with the public `device_panel` tool at that endpoint. Call
   `inspect` until the answer has `connection: "connected"`,
   `image_received: true` and `image_displayed: true`.

   Result: The person sees the isolated desktop in the Device panel.

5. Find the Horizon child of the fixture and compare its executable with the
   frozen hash.

   ```sh
   for pid in $(pgrep -f "$smoke_bin/horizon"); do
     readlink "/proc/$pid/exe"; sha256sum "/proc/$pid/exe"
   done
   ```

   Result: One PID shows the frozen binary and the hash from step 1.

6. Make a directory for the first workspace in the private home of the
   fixture. The fixture masks the home at `<state>/data/home`.

   ```sh
   mkdir -p /tmp/panel-cwd-1/data/home/youpark.no
   ```

   Result: The directory `~/youpark.no` exists in the fixture home.

## 6. Tasks

### 6.1 PWD-1 — A panel starts in the directory of its workspace

1. Press Ctrl and double-click an empty position on the canvas.

   Result: The preset picker opens.

2. Select **Pi cwd demo**.

   Result: The **Select workspace directory** picker opens.

3. Select `~/youpark.no`.

   Result: Horizon makes a new workspace and one agent panel. The panel shows
   `working directory: /home/peters/youpark.no`. The sidebar shows the same
   directory below the workspace name.

### 6.2 PWD-2 — A restart says why the directory is not available

1. Rename the workspace directory in the fixture home.

   ```sh
   mv /tmp/panel-cwd-1/data/home/youpark.no /tmp/panel-cwd-1/data/home/youpark.no-old
   ```

   Result: The directory `~/youpark.no` is gone.

2. Right-click the title bar of the agent panel and select **Restart**.

   Result: The panel shows this text:

   ```text
   Horizon could not start this panel.

   Panel: Pi cwd demo
   Directory: /home/peters/youpark.no
   Reason: the directory does not exist

   Set the directory of this workspace, then restart the panel.
   ```

   The panel does not show a shell prompt, and it does not run a process in the
   directory of the Horizon binary.

3. Read the log of the fixture and find the line with
   `panel cannot start in its working directory`.

   Result: The line gives the panel, the directory and the reason.

### 6.3 PWD-3 — A new panel asks for a directory

1. Press Ctrl and double-click an empty position inside the same workspace.

   Result: The preset picker opens with **Pi cwd demo** and a **Dir** button.

2. Select **Pi cwd demo**.

   Result: The **Select terminal directory** picker opens. It contains
   `~/youpark.no/` in the search field and `~/youpark.no-old` in the result
   list. Horizon does not make a panel before a directory is selected.

### 6.4 PWD-4 — The selected directory becomes the workspace directory

1. Select `~/youpark.no-old`.

   Result: Horizon makes an agent panel. The panel shows
   `working directory: /home/peters/youpark.no-old`. The sidebar shows
   `~/youpark.no-old` below the workspace name.

2. Right-click the title bar of the first panel and select **Restart**.

   Result: That panel also runs again, and it shows
   `working directory: /home/peters/youpark.no-old`.

### 6.5 PWD-5 — A remote panel needs no local directory

1. Add a panel of an SSH preset to a workspace whose directory is gone.

   Result: The SSH panel connects as before. The missing local directory does
   not stop it, because its shell runs on the remote computer.

## 7. Cleanup

1. Click the close button of the window of the fixture.

   Result: The fixture stops and removes `/tmp/panel-cwd-1/data`.

2. Close the Device panel with the public `device_panel` tool, operation
   `close`, only for the panel that this procedure made.

3. Remove the frozen binary directory and the preset file.

   ```sh
   rm -rf "$smoke_bin" /tmp/panel-cwd-presets.json
   ```

## 8. Pass criteria

- PWD-1: The panel shows the directory of its workspace.
- PWD-2: The panel says why it cannot start, and it names the directory.
- PWD-3: Horizon asks for a directory before it makes a panel.
- PWD-4: The selected directory becomes the directory of the workspace, and the
  panels run in it.
- PWD-5: A remote panel connects.

## 9. Report

Record the candidate hash, the fixture display, the Device panel id, the result
of each task and the log lines. Keep the screenshots and the video with the
private evidence of the task.
