---
procedure: offscreen-output-repaint
feature: Repaint rate for terminal output
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Repaint rate for terminal output test procedure

## 1. Purpose

This procedure proves that output from a terminal panel that is not on the
screen does not keep the window at the output frame rate. It also proves that
the panel shows its latest output when it comes back into view.

## 2. Applicability

- Use a Horizon candidate that contains this change. Use the `profiling`
  build profile for CPU values.
- Platforms: Linux, with the local device fixture.
- This procedure does not test browser panels, Device panels or detached
  windows.

## 3. Safety

> **CAUTION: USE ONLY THE ISOLATED DESKTOP.** A drag on the developer desktop can move a real panel.

## 4. Equipment and preconditions

- A frozen Horizon candidate and its SHA-256.
- A frozen `horizon-device` from the same commit.
- The local device fixture with `--native-view`. Read
  [the fixture guide](../../../scripts/device-smoke/README.md).
- A live Device panel in the current workspace.

## 5. Setup

1. Build the candidate.

   ```sh
   cargo build --profile profiling --features trace-profiling
   cargo build -p horizon-device --features cli
   ```

   Result: The build completes without errors.

2. Copy `target/profiling/horizon` and `target/debug/horizon-device` to a new
   directory. Record their SHA-256.

   Result: The directory contains the two files and `SHA256SUMS`.

3. Start the fixture with the frozen candidate and `--native-view`.

   Result: The fixture prints a loopback VNC address.

4. Open that address in a Device panel.

   Result: The panel shows the isolated Horizon window with two terminal
   panels and the FPS meter in the top bar.

5. Find the process ID of the Horizon child in the fixture process tree.
   Compare `sha256sum /proc/<pid>/exe` with `SHA256SUMS`.

   Result: The two values are the same.

6. Close the `Live render heartbeat` panel.

   Result: Only the `Device input test` panel stays on the canvas.

## 6. Tasks

### 6.1 ON — Output from a panel on the screen

1. Type this command in the `Device input test` panel. Then push Enter.

   ```sh
   i=0; while :; do i=$((i+1)); echo "agent output line $i"; sleep 0.01; done
   ```

   Result: The panel shows new lines continuously. After 10 seconds, the FPS
   meter shows a value of more than 10.

2. Measure the CPU time of the Horizon child for 10 seconds.

   ```sh
   a=$(awk '{print $14+$15}' /proc/<pid>/stat); sleep 10
   b=$(awk '{print $14+$15}' /proc/<pid>/stat)
   echo $(( (b - a) * 100 / (10 * $(getconf CLK_TCK)) ))
   ```

   Result: The command prints the CPU percent. Record it as ON.

### 6.2 OFF — Output from a panel that is not on the screen

1. Drag the empty canvas to the right until the `Device input test` panel is
   fully out of view.

   Result: The canvas does not show the `Device input test` panel. The command
   continues to write output.

2. Wait 10 seconds.

   Result: The FPS meter shows 0 or a value of less than 5.

3. Measure the CPU time of the Horizon child for 10 seconds, as in step 6.1.2.

   Result: The value is less than half of ON. Record it as OFF.

### 6.3 BACK — Latest output after the panel comes back

1. Drag the canvas to the left until the `Device input test` panel is fully in
   view.

   Result: The panel shows a line number that is larger than the numbers
   before the drag. The panel does not show old content first.

2. Wait 10 seconds.

   Result: The panel continues to show new lines. The FPS meter shows a value
   of more than 10 again. The meter shows an average, so it increases slowly.

## 7. Pass criteria

- 6.1 shows continuous output and a frame rate of more than 10.
- 6.2 shows a frame rate of less than 5 and OFF is less than half of ON.
- 6.3 shows the latest output immediately after the panel comes back.

## 8. Cleanup

1. Close the Device panel.

   Result: The panel closes. The fixture continues to run.

2. Stop the fixture with Ctrl+C in its terminal.

   Result: The fixture stops Horizon, the output command and its other child
   processes. It removes its target file.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
