---
procedure: cloud-idle-period
feature: An idle period in a profile, which has no effect
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [RunPod API key file, SSH identity file]
owner: peters
---

# Cloud idle period test procedure

## 1. Purpose

This procedure makes sure that `idle_stop_minutes` still parses but has no
effect. **New cloud** and **Rebuild** must show a note for it. A new worker must
not get an idle period. A RunPod worker that was created with an idle period
before this change must still reconnect.

## 2. Applicability

- Candidate: each candidate that changes how a profile, a worker or the cloud
  card handles `idle_stop_minutes`.
- Platforms: Linux. Tasks I01 and I04 use an isolated desktop with a live view.
- Providers: RunPod for tasks I02 and I03.
- This procedure does not test: the agent stop. For the agent stop, use
  [cloud-agent-stop](cloud-agent-stop.md).

## 3. Safety

> **CAUTION:** TASKS I02 AND I03 RENT RUNPOD WORKERS. The provider bills each
> worker while it runs, and bills the storage after the stop.

> **CAUTION:** USE ONLY THE CLOUDS THAT THIS RUN MADE. If you stop or delete
> other clouds, other people lose their work.

> **CAUTION:** DO NOT SHOW THE SETTINGS FILE, THE KEY FILES OR THE WORKER
> ENVIRONMENT IN EVIDENCE. The environment of a RunPod worker contains its stop
> credential.

## 4. Equipment and preconditions

- The [local device smoke fixture](../../../scripts/device-smoke/README.md)
  with `--native-view`, for tasks I01 and I04.
- A frozen candidate and its SHA-256.
- A cloud settings file, `<settings>`, with a RunPod key. Use a reference to
  the key file. Do not write a key in this procedure.
- A synthetic repository with a `.horizon/cloud.yml` file. Its profile `idle`
  is RunPod, CPU, the cheapest offer, an image-only profile on the quick start
  image, `agents: [claude]` and `idle_stop_minutes: 30`.
- The `cloud_deploy` example program, built from the candidate commit, and a
  second one, `<before>`, built from the commit before the change:

  ```bash
  cargo build -p horizon-core --example cloud_deploy --features cloud-workspaces
  ```

In this procedure, `<state root>` is the directory of one cloud.

## 5. Setup

1. Start the fixture with the frozen candidate and a new state directory.

   Result: The Device panel shows the candidate on the isolated desktop.

## 6. Tasks

### 6.1 I01 — New cloud shows the note

1. Open **New cloud** for the synthetic repository.

   Result: Below the profile, the dialog shows `idle_stop_minutes in profile
   idle has no effect: no cloud stops when idle. An agent on the worker stops it
   when its task is done, or choose Stop.`

2. Remove `idle_stop_minutes` from the profile, commit, and open **New cloud**
   again.

   Result: The dialog shows no note about `idle_stop_minutes`.

3. Put `idle_stop_minutes: 30` back in the profile, commit, and close **New
   cloud**.

   Result: The profile `idle` sets `idle_stop_minutes: 30` again for tasks I02
   and I03.

### 6.2 I02 — A new worker gets no idle period

> **CAUTION:** THIS TASK RENTS A RUNPOD WORKER.

1. Deploy a cloud with the profile `idle` and the candidate:

   ```bash
   target/debug/examples/cloud_deploy deploy <settings> <repository> idle <state root> <cloud id>
   ```

   Result: The last line says that the cloud is ready.

2. On the worker, as root, type
   `tr '\0' '\n' < /proc/1/environ | grep -c HORIZON_IDLE_STOP_MINUTES`.

   Result: The output is `0`.

3. Delete the cloud with `cloud_deploy delete <settings> <state root>`.

   Result: The command ends without an error.

### 6.3 I03 — A worker created before the change reconnects

> **CAUTION:** THIS TASK RENTS A RUNPOD WORKER.

1. Deploy a cloud with the profile `idle` and `<before>`.

   Result: The last line says that the cloud is ready.

2. On the worker, as root, type
   `tr '\0' '\n' < /proc/1/environ | grep HORIZON_IDLE_STOP_MINUTES`.

   Result: The output is `HORIZON_IDLE_STOP_MINUTES=30`.

3. Reconnect the cloud with the candidate:

   ```bash
   target/debug/examples/cloud_deploy reconnect <settings> <state root>
   ```

   Result: The last line says that the cloud is ready. The worker ID is the
   same as in step 1.

4. Stop the cloud, resume it and reconnect it with the candidate:

   ```bash
   target/debug/examples/cloud_deploy stop <settings> <state root>
   target/debug/examples/cloud_deploy resume <settings> <state root>
   target/debug/examples/cloud_deploy reconnect <settings> <state root>
   ```

   Result: The cloud is ready again with the same worker ID.

5. Delete the cloud with `cloud_deploy delete <settings> <state root>`.

   Result: The command ends without an error.

### 6.4 I04 — Rebuild shows the note and no change

1. Use a ready cloud of a profile with a `build` section. Add
   `idle_stop_minutes: 30` to the profile in the working copy.
2. Choose **Rebuild** on the card.

   Result: The confirmation shows the note of task I01. The list of changes does
   not name `idle_stop_minutes`.

3. Choose **Keep current image**.

   Result: Nothing is rebuilt.

## 7. Pass criteria

- I01 to I04 show each `Result:` line.
- No worker of this run gets `HORIZON_IDLE_STOP_MINUTES` from the candidate.

## 8. Cleanup

> **CAUTION:** DELETE ONLY THE CLOUDS THAT THIS RUN MADE. A delete removes the
> worker and its storage. You cannot undo it.

1. List the pods and network volumes of the RunPod account through its API.

   Result: No pod or volume of this run remains.

2. Close the candidate window normally and stop the fixture.

   Result: The fixture removes its display and its processes.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
