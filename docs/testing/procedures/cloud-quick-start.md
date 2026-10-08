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
a RunPod cloud on the base image. The run uses no registry login, builds no
image and uses no Docker on this computer. The procedure also makes sure of these
items:

- **New cloud** shows a missing configuration as guidance, not as an error.
- The first **Add panel** after **Ready** works.
- Each segment of the stage track shows its hover text.
- **Rebuild image & restart** moves a quick start cloud to the pinned image and
  keeps its workspace.

## 2. Applicability

- Candidate: each candidate that changes quick start, the quick start pin or its
  kept contract result, the base image recipe, the New cloud repository step,
  the stage track of the cloud card or the rebuild of a cloud.
- Platforms: Linux, in an isolated desktop with a live view.
- Providers: the RunPod lane.
- This procedure does not test: the Hetzner lane, the setup agent, a GPU
  worker, the rebuild of a cloud with a `build` section or the CI job that
  publishes the base image.

## 3. Safety

> **CAUTION:** TASKS Q04 TO Q06 RENT A RUNPOD WORKER AND A NETWORK VOLUME. The
> provider bills the worker while it runs and the volume until you delete it.

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
- A directory, `<no-docker-path>`, with links to each program on `PATH` except
  `docker`. Start the fixture and each candidate with `PATH=<no-docker-path>` and
  `DOCKER_HOST=unix:///nonexistent/docker.sock`. Thus the candidate cannot start
  Docker, also if this computer has Docker.
- A second candidate, `<earlier>`, for task Q06. Build it from the candidate commit
  with one change: the `image!` macro in `quick_start.rs` names an earlier pin of
  the base image. The earlier pin must still pull. Its worker check must report the
  same markers as `CONTRACT`, because `<earlier>` keeps that value. Freeze
  `<earlier>` and write down its SHA-256.
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

1. Start the fixture with the frozen candidate, a new state directory,
   `PATH=<no-docker-path>` and `DOCKER_HOST=unix:///nonexistent/docker.sock`.

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

   Result: The output shows the line `Worker contract: the pinned quick-start
   image passed this check when it was published`. It shows no stage `Build
   locally` or `Push image` and no image download. The command ends with
   `Prepared:` and the pinned reference.

3. Do step 2 again with `PATH=<no-docker-path>` and
   `DOCKER_HOST=unix:///nonexistent/docker.sock`.

   Result: The output is the same as in step 2.

4. Remove `/tmp/q03-state`.

   Result: The directory does not exist.

### 6.4 Q04 — Start the cloud on RunPod

> **CAUTION:** THIS TASK RENTS A RUNPOD WORKER AND A NETWORK VOLUME. Write each
> provider resource in the resource ledger.

1. Examine the environment of the candidate process:

   ```bash
   tr '\0' '\n' < /proc/<candidate PID>/environ | grep -E '^(PATH|DOCKER_HOST)='
   ```

   Result: `PATH` is `<no-docker-path>` and `DOCKER_HOST` names no socket.

2. Type this command:

   ```bash
   PATH=<no-docker-path> command -v docker || echo absent
   ```

   Result: The command shows `absent`.

3. In the New cloud dialog of Q02, select the smallest CPU worker.

   Result: The summary shows the compute price for each hour.

4. Click **Start cloud** and write down the time.

   Result: The cloud card shows **Validating**.

5. Examine the steps of the card while it validates.

   Result: **Build locally** and **Push image** show `skipped`. They do not show
   a check mark.

6. Write the new pod and the new network volume in the resource ledger.

   Result: The ledger has the IDs of the two resources.

7. Wait for **Ready** and write down the time.

   Result: The card shows **Ready** and the time to ready. The output of the
   card has the line `Worker contract: the pinned quick-start image passed this
   check when it was published`. It has no image download.

8. Examine the stage track of the card.

   Result: The segments of **Build locally** and **Push image** show a thin
   rule, not a colour.

9. Put the pointer on the segment of **Build locally** and wait one second.

   Result: The hover text is `Build locally · skipped`. The text
   `Click the title to rename. Drag to move this cloud.` does not show.

10. Put the pointer on the segment of **Provision worker** and wait one second.

    Result: The hover text is `Provision worker`.

11. Put the pointer on the title of the card and wait one second.

    Result: The hover text is `Click the title to rename. Drag to move this cloud.`

12. In the RunPod console, examine the pod of this cloud.

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

### 6.6 Q06 — Rebuild the quick start cloud

> **CAUTION:** THIS TASK RESTARTS THE WORKER TWO TIMES. The worker continues to
> cost money while it runs.

1. Type this command in a worker shell:

   ```bash
   echo q06-marker > /workspace/q06-marker && cat /workspace/q06-marker
   ```

   Result: The shell shows `q06-marker`.

2. On the card, open the details and click the **Manage** tab.

   Result: The Manage tab shows **Rebuild image & restart…**.

3. Click **Rebuild image & restart…**.

   Result: The card asks `Restart the worker on the base image that this Horizon
   version pins?`. The text does not name a committed recipe.

4. Click **Rebuild and restart**.

   Result: The card shows `Image unchanged; nothing to restart` and **Ready**.
   The worker does not restart.

5. Close the candidate window normally.

   Result: The candidate process stops. The fixture stays.

6. Start `<earlier>` on the same fixture state, with `PATH=<no-docker-path>` and
   `DOCKER_HOST=unix:///nonexistent/docker.sock`.

   Result: The board shows the card of the cloud.

7. If the card does not show **Ready**, click **Reconnect**.

   Result: The card shows **Ready**.

8. In the **Manage** tab, click **Rebuild image & restart…**.

   Result: The card asks for confirmation.

9. Click **Rebuild and restart** and write down the time.

   Result: The track shows **Build locally** and **Push image** as skipped. The
   card shows **Replace image**, then **Ready**.

10. In the RunPod console, examine the image of the pod.

    Result: The image is the earlier pin. The pod ID and the network volume did
    not change.

11. Open a worker shell and type `cat /workspace/q06-marker`.

    Result: The shell shows `q06-marker`.

12. Close `<earlier>` normally.

    Result: The `<earlier>` process stops.

13. Start the candidate again on the same state, with `PATH=<no-docker-path>` and
    `DOCKER_HOST=unix:///nonexistent/docker.sock`.

    Result: The board shows the card of the cloud.

14. Do steps 7 to 11 again.

    Result: The pod image is the pin of the candidate. The pod ID, the network
    volume and the marker did not change.

## 7. Pass criteria

- Q01 shows the guidance section and no red error.
- Q02 loads the profile `quick-start` with the pinned image.
- Q03 refuses `<configured>` and prepares `<plain>` with no build and no push.
- Q03 and Q04 run with no `docker` program and no Docker daemon for the
  candidate.
- Q04 reaches **Ready** on the pinned digest with no registry login. **Build
  locally** and **Push image** show as skipped. Each stage segment shows its own
  hover text, and the card title shows the card hint.
- Q05 opens each worker shell the first time.
- Q06 reports an unchanged image for the same pin. It switches the pod to the
  other pin and back, and keeps the pod, the volume and `/workspace`. No candidate
  in Q06 can start Docker.
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
