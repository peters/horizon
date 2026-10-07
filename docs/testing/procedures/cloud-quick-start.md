---
procedure: cloud-quick-start
feature: Quick start on the base image for a repository without cloud.yml
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [RunPod API key file, SSH identity file]
owner: peters
---

# Cloud quick start test procedure

## 1. Purpose

This procedure makes sure that a repository without `.horizon/cloud.yml` starts
a RunPod cloud on the base image. The run uses no registry login and builds no
image. The procedure also makes sure that **New cloud** shows a missing
configuration as guidance, not as an error, and that the first **Add panel**
after **Ready** works.

## 2. Applicability

- Candidate: each candidate that changes quick start, the quick start pin, the
  base image recipe, the New cloud repository step or the stage track of the
  cloud card.
- Platforms: Linux, in an isolated desktop with a live view.
- Providers: the RunPod lane.
- This procedure does not test: the Hetzner lane, the setup agent, a GPU
  worker, **Rebuild image & restart** or the CI job that publishes the base
  image.

## 3. Safety

> **CAUTION:** TASK Q04 RENTS A RUNPOD WORKER AND A NETWORK VOLUME. The provider
> bills the worker while it runs and the volume until you delete it.

> **CAUTION:** DELETE ONLY THE CLOUD THAT THIS RUN MADE. If you delete other
> clouds or volumes, other people lose their work.

> **CAUTION:** DO NOT SHOW THE SETTINGS FILE OR THE KEY FILES IN EVIDENCE. A
> recording or a screenshot can show a secret. Do not open a settings view that
> shows a key while the recorder runs.

## 4. Equipment and preconditions

- The [local device smoke fixture](../../../scripts/device-smoke/README.md)
  with `--native-view`.
- A frozen candidate and its SHA-256.
- A Device panel that shows a live view of the fixture.
- A private evidence directory, `<evidence>`, outside the fixture state.
- A private resource ledger.
- A cloud settings file, `<settings>`, in the private home of the fixture, with
  these values:
  - A reference to the RunPod API key file.
  - A reference to the SSH identity file.
  - `docker_config`: an empty directory. The run must use no registry login.
  - `registry_pull_auth_id`: `null`.

  Do not write a key in this procedure.
- Docker with buildx on the computer that runs the fixture.
- A synthetic repository, `<plain>`, with one commit and no `.horizon`
  directory. A clone of `https://github.com/octocat/Hello-World` is
  satisfactory.
- A synthetic repository, `<configured>`, with a committed `.horizon/cloud.yml`
  that has one RunPod CPU profile.
- The `cloud_deploy` example program, built from the candidate commit:

  ```bash
  cargo build -p horizon-core --example cloud_deploy
  ```

- The pinned digest of the base image. Read it from the `image!` macro in
  `crates/horizon-core/src/cloud_runtime/repository/launch/quick_start.rs`.

## 5. Setup

1. Start the fixture with the frozen candidate and a new state directory.

   Result: The Device panel shows the candidate on the isolated desktop.

2. Start a recorder on the display of the fixture.

   Result: The recorder writes a video file in `<evidence>`.

3. Open a workspace in `<plain>` in a persistent session.

   Result: The board shows the workspace.

## 6. Tasks

### 6.1 Q01 — Guidance for a commit without cloud.yml

1. Click **Cloud** in the menu bar.

   Result: The Cloud menu opens.

2. Click **New cloud…**.

   Result: The New cloud dialog opens and reads `<plain>`.

3. Examine the dialog below the cloud title.

   Result: An open section shows **This commit has no .horizon/cloud.yml**. It
   shows **Quick start on the public base image** and **Open setup agent**.

4. Examine the dialog for a red error.

   Result: The dialog shows no red error. **Start cloud** stays unavailable.

### 6.2 Q02 — Load the quick start profile

1. Type a cloud title, for example `Quick start test`.

   Result: The title field shows the text.

2. Click **Quick start on the public base image**.

   Result: The dialog shows the profile `quick-start` with 2 vCPU and 4 GB.

3. Read the line under the profile.

   Result: The line names `ghcr.io/peters/horizon-worker-base` and the first
   12 characters of the pinned digest.

4. Open **More options** and click **Read .horizon/cloud.yml**.

   Result: The dialog shows **This commit has no .horizon/cloud.yml** again.

5. Click **Quick start on the public base image** again.

   Result: The dialog shows the profile `quick-start` again.

### 6.3 Q03 — Refuse quick start for a commit with cloud.yml

This task allocates nothing.

1. Type this command in a terminal outside the fixture:

   ```bash
   target/debug/examples/cloud_deploy prepare-image <settings> <configured> quick-start /tmp/q03-state q03 --quick-start
   ```

   Result: The command stops with
   `This commit has its own .horizon/cloud.yml. Quick start is only for a repository without one.`

2. Type this command:

   ```bash
   target/debug/examples/cloud_deploy prepare-image <settings> <plain> quick-start /tmp/q03-state q03 --quick-start
   ```

   Result: The output shows the stage `Validate` and the worker contract
   markers. It shows no stage `Build locally` or `Push image`. The command
   ends without an error.

3. Remove `/tmp/q03-state`.

   Result: The directory does not exist.

### 6.4 Q04 — Start the cloud on RunPod

> **CAUTION:** THIS TASK RENTS A RUNPOD WORKER AND A NETWORK VOLUME. Write each
> provider resource in the resource ledger.

1. In the New cloud dialog of Q02, select the smallest CPU worker.

   Result: The summary shows the compute price for each hour.

2. Click **Start cloud** and write down the time.

   Result: The cloud card shows **Validating**.

3. Examine the steps of the card while it validates.

   Result: **Build locally** and **Push image** show `skipped`. They do not show
   a check mark.

4. Write the new pod and the new network volume in the resource ledger.

   Result: The ledger has the IDs of the two resources.

5. Wait for **Ready** and write down the time.

   Result: The card shows **Ready** and the time to ready.

6. Examine the stage track of the card.

   Result: The segments of **Build locally** and **Push image** show a thin
   rule, not a colour. Their hover text ends with `skipped`.

7. In the RunPod console, examine the pod of this cloud.

   Result: The image is `ghcr.io/peters/horizon-worker-base@sha256:` with the
   pinned digest. The pod has no registry login.

### 6.5 Q05 — Add a panel right after Ready

1. Immediately after **Ready**, open the panel picker of the cloud.

   Result: The panel picker **Add panel** opens.

2. Click **Shell**.

   Result: A worker shell opens. Horizon shows no message
   `Another controller owns this cloud operation`.

3. Type this command in the worker shell:

   ```bash
   pwd && git log -1 --format=%h
   ```

   Result: The path is `/workspace/checkout`. The commit is the `HEAD` of
   `<plain>`.

4. Wait two minutes. Then open one more worker shell.

   Result: The second worker shell opens the first time.

## 7. Pass criteria

- Q01 shows the guidance section and no red error.
- Q02 loads the profile `quick-start` with the pinned image.
- Q03 refuses `<configured>` and prepares `<plain>` with no build and no push.
- Q04 reaches **Ready** on the pinned digest with no registry login. **Build
  locally** and **Push image** show as skipped.
- Q05 opens each worker shell the first time.
- Cleanup removes the pod and the network volume of this run.

## 8. Cleanup

> **CAUTION:** DELETE ONLY THE CLOUD THAT THIS RUN MADE. A delete removes the
> worker and its storage. You cannot undo it.

1. Stop the recorder.

   Result: The video file in `<evidence>` plays.

2. On the card of this cloud, close the cloud with its workspace storage.

   Result: The card shows **Worker deleted** and **Workspace storage cleaned up**.

3. In the RunPod console, look for the pod and the volume in the resource ledger.

   Result: The console does not show them.

4. Close the candidate window normally.

   Result: The candidate process stops.

5. Stop the fixture.

   Result: The fixture removes its display and its processes.

6. Remove each copy of a key file from the fixture state.

   Result: The fixture state has no key file.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
