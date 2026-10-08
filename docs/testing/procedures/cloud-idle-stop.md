---
procedure: cloud-idle-stop
feature: Cloud idle stop and the stopped card
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [RunPod API key file, Hetzner token file, SSH identity file]
owner: peters
---

# Cloud idle stop test procedure

## 1. Purpose

This procedure makes sure that an idle stop on RunPod and on Hetzner shows a
stopped cloud with its cause. The card must not show **Operation failed** for a
stop that the owner did not start. **Resume worker** must start the cloud again.

## 2. Applicability

- Candidate: each candidate that changes the idle stop, the provider check or
  the cloud card status.
- Platforms: Linux, in an isolated desktop with a live view.
- Providers: the RunPod lane and the Hetzner lane.
- This procedure does not test: an agent stop with `horizon-worker-stop`, a
  shared worker, or a profile with hosted devices.

## 3. Safety

> **CAUTION:** EACH LANE RENTS COMPUTE FOR ONE HOUR OR MORE. The provider bills
> the worker while it runs, and bills the storage after the stop.

> **CAUTION:** USE ONLY THE CLOUDS THAT THIS RUN MADE. If you stop or delete
> other clouds, other people lose their work.

> **CAUTION:** DO NOT SHOW THE SETTINGS FILE OR THE KEY FILES IN EVIDENCE. A
> recording or a screenshot can show a secret.

## 4. Equipment and preconditions

- The [local device smoke fixture](../../../scripts/device-smoke/README.md)
  with `--native-view`.
- A frozen candidate and its SHA-256.
- A Device panel that shows a live view of the fixture.
- A private evidence directory, `<evidence>`, outside the fixture state.
- A cloud settings file, `<settings>`, with a RunPod key and a Hetzner token.
  Use references to the key files. Do not write a key in this procedure.
- A repository with a `.horizon/cloud.yml` file. It has these two profiles:
  - `runpod-idle`: RunPod, CPU, an image from `examples/cloud-worker`, and
    `idle_stop_minutes: 30`.
  - `hetzner-idle`: Hetzner, an image from `examples/cloud-worker`, and
    `idle_stop_minutes: 30`.
- A worker image that reports `horizon-idle-report-contract=1`. Build the image
  from the current `examples/cloud-worker` directory.
- The `cloud_deploy` example program, built from the candidate commit:

  ```bash
  cargo build -p horizon-core --example cloud_deploy
  ```

- A clock. Write down the time of each result.

In this procedure, `<state root>` is the directory of one cloud under
`<home>/.horizon/cloud/`. `<home>` is the private home of the fixture.

## 5. Setup

1. Start the fixture with the frozen candidate and a new state directory.

   Result: The Device panel shows the candidate on the isolated desktop.

2. Start a recorder on the display of the fixture.

   Result: The recorder writes a video file in `<evidence>`.

3. Open the repository in a persistent session.

   Result: The board shows the workspace of the repository.

## 6. Tasks

Do the RunPod tasks and the Hetzner tasks in two different clouds. You can do
the two lanes at the same time.

### 6.1 R01 — RunPod idle stop while Horizon runs

> **CAUTION:** THIS TASK RENTS A RUNPOD WORKER. Delete the cloud in the cleanup
> if you do not need it again.

1. Make a new cloud with the profile `runpod-idle`.

   Result: The card shows **Ready**.

2. Open one shell panel in the cloud.

   Result: The shell panel shows a prompt on the worker.

3. Type this command in the shell panel:

   ```bash
   tail -n 3 /workspace/idle.log
   ```

   Result: The idle log shows no stop line.

4. Close the shell panel. Do not type in any panel of the cloud.

   Result: No agent terminal in the cloud prints output.

5. Write down the time.

   Result: You have the start time of the idle period.

6. Wait 30 minutes or more. Do not close Horizon.

   Result: After 30 to 35 minutes, the card shows **Checking provider** for some
   seconds.

7. Examine the card header.

   Result: The card shows **Stopped after 30 idle minutes** and
   **Storage kept · billable**. The header button is **Resume worker**.

8. Examine the card header again.

   Result: The card does not show **Operation failed**. The card does not show
   **Local operation timed out**.

9. Open the **Status** tab of the card and read its output.

   Result: The last line starts with
   `No agent activity for 30 minutes, so this worker stopped itself.` The line
   ends with `Resume starts the same worker again.`

10. Type this command in a terminal outside the fixture:

    ```bash
    target/debug/examples/cloud_deploy reconcile <settings> <state root>
    ```

    Result: The first line contains `"status":"inactive"`. The second line is
    `Stopped. Resume starts the same worker again.`

11. Click **Resume worker**.

    Result: The card shows **Ready** in about one minute. The card shows the
    same worker ID as before.

12. Open one shell panel in the cloud.

    Result: The checkout and the tailnet IP are the same as before the stop.

13. Type this command in the shell panel:

    ```bash
    tail -n 3 /workspace/idle.log
    ```

    Result: The idle log contains
    `No agent activity for 30 minutes; stopping this worker`.

### 6.2 R02 — RunPod idle stop while Horizon is closed

1. Close the shell panel. Do not type in any panel of the cloud.

   Result: No agent terminal in the cloud prints output.

2. Close the candidate window normally.

   Result: The candidate process stops. The fixture continues.

3. Wait 35 minutes or more.

   Result: The worker stops itself during this time.

4. Start the candidate again with the same state directory.

   Result: The card shows **Checking provider** for some seconds after a
   reconnect attempt.

5. Examine the card header.

   Result: The card shows **Stopped**, **Storage kept · billable** and
   **Stopped outside Horizon**. The header button is **Resume worker**.

6. Examine the card header again.

   Result: The card does not show **Operation failed**, **Provisioning failed**
   or **Reconnect**.

7. Open the **Status** tab of the card and read its output.

   Result: The last line starts with
   `The provider reports that this worker is stopped. Horizon did not stop it;`

8. Click **Resume worker**.

   Result: The card shows **Ready**.

### 6.3 R03 — RunPod stop from the provider console

1. Open one agent panel in the cloud. Type a short prompt.

   Result: The agent prints output. The worker is active.

2. Stop the worker in the RunPod console.

   Result: The RunPod console shows the worker as `EXITED`.

3. Wait five minutes or less.

   Result: The card shows **Stopped** and **Stopped outside Horizon**.

4. Examine the card header.

   Result: The card does not show **Stopped after 30 idle minutes**. The
   card does not show **Operation failed**.

### 6.4 H01 — Hetzner idle stop while Horizon runs

> **CAUTION:** THIS TASK RENTS A HETZNER SERVER AND A VOLUME. Delete the cloud
> in the cleanup if you do not need it again.

1. Make a new cloud with the profile `hetzner-idle`.

   Result: The card shows **Ready**.

2. Do not type in any panel of the cloud. Write down the time.

   Result: You have the start time of the idle period.

3. Wait 30 minutes or more. Do not close Horizon.

   Result: After 30 to 35 minutes, the card changes.

4. Examine the card header.

   Result: The card shows **Stopped after 30 idle minutes** and
   **Storage kept · billable**. The header button is **Resume worker**.

5. Open the **Status** tab of the card and read its output.

   Result: The last line starts with `No agent activity for <N> minutes, so
   Horizon stopped this cloud.` `<N>` is the measured idle time. It is 30 or
   more, because the idle watch reads the worker every few minutes.

6. Type this command in a terminal outside the fixture:

   ```bash
   target/debug/examples/cloud_deploy reconcile <settings> <state root>
   ```

   Result: The command prints the provider status. The second line starts with
   `Stopped. Resume creates a new server`.

7. Click **Resume worker**.

   Result: The card shows **Ready**. The checkout on the volume is the same.

## 7. Pass criteria

- R01, R02, R03 and H01 show each `Result:` line.
- No task shows **Operation failed** after a stop that the owner did not start.
- **Resume worker** brings each cloud back to **Ready**.

## 8. Cleanup

> **CAUTION:** DELETE ONLY THE CLOUDS THAT THIS RUN MADE. A delete removes the
> worker and its storage. You cannot undo it.

1. Stop the recorder.

   Result: The video file in `<evidence>` plays.

2. For each cloud of this run, choose **Delete cloud resources…** in **Manage…**.

   Result: The card shows **Worker deleted** and **Workspace storage cleaned up**.

3. Close the candidate window normally.

   Result: The candidate process stops.

4. Stop the fixture.

   Result: The fixture removes its display and its processes.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
