---
procedure: cloud-panels-g-companions
feature: Cloud panels smoke test, area G (companion repositories)
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [registry push credential for the build profile]
owner: peters
---

# Cloud panels test procedure, area G: companion repositories

## 1. Purpose

This area makes sure that a cloud can use a sibling and a companion cloud. It
also makes sure that agents can start and stop a companion cloud through MCP.

## 2. Applicability

- Candidate: the frozen candidate of [area S](s-test-fixture.md).
- Platforms: Linux. Providers: Hetzner for `smoke-a` and `smoke-lib`, RunPod
  for `smoke-sib`.
- This area does not test: credential isolation between companion clouds. These
  clouds share trusted shell access. See
  [companion access](../../../cloud-workspaces.md#companion-access-in-cloud-panels).

## 3. Safety

> **CAUTION:** CHECK A COMPANION ONLY ON A TEST CLOUD. A checked companion gives
> the source cloud shell access to the target worker.

> **CAUTION:** RECORD EACH SERVER, POD AND VOLUME IN THE RESOURCE LEDGER. An
> MCP request or a resume can make a new Hetzner server.

## 4. Equipment and preconditions

- The equipment in the [main procedure](../cloud-panels.md#4-equipment-and-preconditions).
- The synthetic repository `<lib>`. Its origin is a GitHub URL of the test
  account, for example `https://github.com/<test-owner>/lib.git`. The repository
  does not need to exist on GitHub.
- The synthetic repository `<sib>`, with a GitHub origin of the test account
  in the same form.
- `<lib>` and `<sib>` contain the committed `.horizon/cloud.yml` from
  [area B](b-repository-configuration.md). It has the profiles `hetzner-cpu`
  and `runpod-build`. A sibling needs a build profile.
- `<sib>` contains a committed Git LFS file and a pinned submodule. If it does
  not, add them as in the [setup of area D](d-deployment.md#5-setup), and commit
  them before G02.
- The committed `.horizon/cloud.yml` of `<repo>` declares the companion `lib`
  with the profile `hetzner-cpu` and the sibling `sib` with the profile
  `runpod-build` and `placement: same_worker`. Area B commits this file.
- The registry push credential for the `runpod-build` profile, in the cloud settings.
- A local agent panel in the workspace of `smoke-a`. The agent uses the MCP
  server of the candidate.
- The rules of the [companion controller](../../../companion-controller.md) and
  the [companion worker transport](../../../companion-worker-protocol.md).

## 5. Setup

1. Make sure that `smoke-a` shows **Ready**.

   Result: The source cloud runs.

2. Open **Cloud › New cloud…** in the workspace of `smoke-a`.

   Result: The New cloud dialog opens.

3. Type the path `<lib>` as the repository and `smoke-lib` as the title.

   Result: The dialog reads `<lib>`.

4. Select the profile `hetzner-cpu` and the tailnet **None**.

   Result: The summary shows a Hetzner worker.

   > **CAUTION:** THIS STEP RENTS COMPUTE. Record the server and the volume of
   > `smoke-lib` in the resource ledger.

5. Click **Start cloud**.

   Result: The card of `smoke-lib` shows the deployment stages.

6. Wait until the card of `smoke-lib` shows **Ready**.

   Result: The companion cloud runs.

7. Write the server ID and the volume ID of `smoke-lib` in the resource ledger.

   Result: The ledger contains the resources of `smoke-lib`.

## 6. Tasks

### 6.1 G01 — Show the declared companions on the source card

1. Open the card of `smoke-a`.

   Result: The card shows the section **Companion clouds**.

2. Examine the rows of the section.

   Result: The section shows one row for `lib` with **Not selected**. It does not
   show `sib`, because a sibling is not a companion cloud.

### 6.2 G02 — Start a cloud with a sibling at the pinned commit

1. In `<sib>`, record the commit of `HEAD`.

   ```sh
   git -C <sib> rev-parse HEAD
   ```

   Result: You have the commit of the sibling.

2. Open **Cloud › New cloud…** in the workspace of `smoke-a`.

   Result: The New cloud dialog opens.

3. Type the path `<repo>` as the repository and `smoke-sib` as the title.

   Result: The dialog shows **Siblings on this worker** with a row for `sib`.

4. Select the profile `runpod-build`.

   Result: The summary shows a RunPod worker.

5. Make sure that the row of `sib` shows the path `<sib>`.

   Result: If the path is empty, type `<sib>` or click **Browse…**.

6. Click the checkbox of `sib`.

   Result: The row shows the commit from step 1. **Start cloud** is enabled.

   > **CAUTION:** THIS STEP RENTS COMPUTE AND PUSHES AN IMAGE TO THE REGISTRY.
   > Record the pod and the network volume in the resource ledger.

7. Click **Start cloud**.

   Result: The card shows the image build, the push and the deployment stages.

8. Wait until the card of `smoke-sib` shows **Ready**.

   Result: The card lists `sib` with the commit from step 1.

9. Write the pod ID and the network volume ID in the resource ledger.

   Result: The ledger contains the resources of `smoke-sib`.

10. In the worker shell of `smoke-sib`, show the commit of the sibling.

    ```sh
    git -C ../sib rev-parse HEAD
    ```

    Result: The commit is the same as in step 1.

### 6.3 G03 — Select a companion cloud and use its SSH alias

1. On the card of `smoke-a`, click the checkbox of `lib`.

   Result: The row shows **Checking SSH access…**. If two clouds match, the row
   shows **Choose cloud**.

2. If the row shows **Choose cloud**, select the cloud ID of `smoke-lib`.

   Result: The row shows the ID of `smoke-lib`.

3. Wait until the row shows **Ready · SSH verified**.

   Result: The source cloud can reach the target cloud over SSH.

4. In the worker shell of `smoke-a`, run Git in the companion.

   ```sh
   ssh companion-lib git status
   ```

   Result: The output shows a clean worktree of `<lib>`.

### 6.4 G04 — Examine the companion worktree

1. In the worker shell of `smoke-a`, show the current directory of the companion.

   ```sh
   ssh companion-lib pwd
   ```

   Result: The path starts with `/workspace/companions/worktrees/`.

2. Record the last path component as the grant ID in the private evidence.

   Result: You have the grant ID for G05.

3. In the worker shell of `smoke-lib`, list the companion worktrees.

   ```sh
   ls /workspace/companions/worktrees
   ```

   Result: The list contains the grant ID from step 2.

### 6.5 G05 — Clear the companion

1. On the card of `smoke-a`, clear the checkbox of `lib`.

   Result: The row shows **Removing access…**, then **Not selected**.

2. In the worker shell of `smoke-a`, try the SSH alias.

   ```sh
   ssh companion-lib true
   ```

   Result: SSH refuses the alias or the key. The source cannot reach the target.

3. In the worker shell of `smoke-lib`, list the companion worktrees.

   ```sh
   ls /workspace/companions/worktrees
   ```

   Result: The clean worktree of the grant is gone.

### 6.6 G06 — Clear a companion while the target is stopped

1. Select `lib` again on the card of `smoke-a`, as in G03.

   Result: The row shows **Ready · SSH verified**.

   > **CAUTION:** STOP ONLY `smoke-lib`. On Hetzner, the stop deletes the server and
   > ends all processes on the worker.

2. Click **Stop worker…** on the card of `smoke-lib`.

   Result: The card asks for a second click on **Stop worker**.

3. In the card, click **Stop worker**.

   Result: The card of `smoke-lib` shows **Stopped**.

4. Record the deletion of the server of `smoke-lib` in the resource ledger.

   Result: The ledger shows the server as deleted. The volume stays.

5. On the card of `smoke-a`, clear the checkbox of `lib`.

   Result: The row shows **Access removal pending · original worker must be reachable**.

   > **CAUTION:** THIS STEP RENTS COMPUTE. On Hetzner, the resume makes a new
   > server. Record it in the resource ledger.

6. Click **Resume worker** on the card of `smoke-lib`.

   Result: The card offers **Reconnect cloud**, or it starts the reconnect.

7. If the card shows **Reconnect cloud**, click **Reconnect cloud**.

   Result: The card of `smoke-lib` shows **Ready**.

8. Write the new server ID of `smoke-lib` in the resource ledger.

   Result: The ledger contains the new server.

9. Click **Refresh** in the **Companion clouds** section of `smoke-a`.

   Result: The row of `lib` shows **Not selected**. The removal is complete.

### 6.7 G07 — Use the companion MCP tools

1. Select `lib` again on the card of `smoke-a`, as in G03.

   Result: The row shows **Ready · SSH verified**.

2. In the local agent panel, call `cloud_companions`.

   Result: The answer lists `smoke-a`, the alias `lib`, its checked state, its
   status and the cloud ID of `smoke-lib`.

3. Call `cloud_companion_ensure_ready` with the source cloud and the alias.

   ```json
   {"cloud":"<smoke-a cloud ID>","alias":"lib"}
   ```

   Result: The answer gives an `operation_id` and a phase at once.

4. Call `cloud_companion_operation` with the same `cloud`, `alias` and `operation_id` until `done` is true.

   Result: The last phase is Ready. The companion that runs did not start a new worker.

   > **CAUTION:** STOP ONLY THE TEST COMPANION. On Hetzner, the stop deletes the
   > server of `smoke-lib`.

5. Call `cloud_companion_stop` with the source cloud and the alias.

   Result: The answer gives an `operation_id` and a phase.

6. Call `cloud_companion_operation` with the new `operation_id` until `done` is true.

   Result: The card of `smoke-lib` shows **Stopped**.

7. Record the deletion of the server of `smoke-lib` in the resource ledger.

   Result: The ledger shows the server as deleted.

   > **CAUTION:** THIS STEP RENTS COMPUTE. The request makes a new Hetzner server
   > for `smoke-lib`.

8. Call `cloud_companion_ensure_ready` with the source cloud and the alias.

   Result: The answer gives a new `operation_id`.

9. Call `cloud_companion_operation` until `done` is true.

   Result: The last phase is Ready. The card of `smoke-lib` shows **Ready**.

10. Write the new server ID of `smoke-lib` in the resource ledger.

    Result: The ledger contains the new server.

11. Send the same `cloud_companion_ensure_ready` request again with the `operation_id` of step 8.

    Result: The answer gives the same operation. No second worker starts.

### 6.8 G08 — Refuse a companion that never started

1. Clear the checkbox of `lib` on the card of `smoke-a`.

   Result: The row shows **Not selected**.

2. Open **Cloud › New cloud…**, type `<lib>` as the repository and `smoke-lib0` as the title.

   Result: The dialog reads `<lib>`.

3. Select the profile `hetzner-cpu`.

   Result: The summary shows a Hetzner worker.

   > **CAUTION:** CANCEL THE OPERATION BEFORE THE PROVIDER REQUEST. If the card
   > shows a server, record it in the resource ledger and delete it in X01.

4. Click **Start cloud**.

   Result: The card of `smoke-lib0` shows the first stages.

5. While the card shows the image stage, click **Cancel operation**.

   Result: The card keeps `smoke-lib0` without a worker.

6. Click the checkbox of `lib` on the card of `smoke-a`.

   Result: The row shows **Choose cloud**, because two clouds match.

7. Select the cloud ID of `smoke-lib0`.

   Result: The row shows the ID of `smoke-lib0`.

8. In the local agent panel, call `cloud_companion_ensure_ready` with the source cloud and the alias.

   Result: The answer is `confirmation_required`. No worker starts.

9. Clear the checkbox of `lib` on the card of `smoke-a`.

   Result: The row shows **Not selected**.

10. Select `lib` again and choose the cloud ID of `smoke-lib`.

    Result: The row shows **Ready · SSH verified**.

### 6.9 G09 — List and inspect companions on the worker

1. In the worker shell of `smoke-a`, list the companions.

   ```sh
   horizon-cloud-worker companions list
   ```

   Result: The output shows `lib`, its status Ready, the SSH alias and the worktree.

2. Inspect the companion.

   ```sh
   horizon-cloud-worker companions inspect lib
   ```

   Result: The output shows that the SSH connection and the worktree work.

3. In the agent panel of `smoke-a`, ask the agent to call `cloud_companions_list`.

   Result: The answer shows the same alias and status as step 1.

   > **CAUTION:** PAUSE ONLY THE CANDIDATE CHILD OF THE FIXTURE. If you pause another
   > Horizon, the work of other people stops.

4. Pause the candidate child from S04.

   ```sh
   kill -STOP <child-pid>
   ```

   Result: The Device panel shows a static image. The candidate sends no update to the workers.

5. After 70 seconds, list the companions in a worker shell of `smoke-a`.

   ```sh
   horizon-cloud-worker companions list
   ```

   Result: The status of `lib` is not Ready. Ready observations expire after 60 seconds.

6. Inspect the companion again.

   ```sh
   horizon-cloud-worker companions inspect lib
   ```

   Result: The inspection reaches the companion without the candidate.

7. Continue the candidate child.

   ```sh
   kill -CONT <child-pid>
   ```

   Result: The `frame_sequence` of the Device panel increases again.

8. Wait until the row of `lib` on the card of `smoke-a` shows **Ready · SSH verified**.

   Result: The candidate publishes a new observation. `companions list` shows Ready again.

### 6.10 G10 — Use the companion tools through a run plan

1. Build the `horizon-browser` CLI in the worktree of S01.

   ```sh
   cargo build -p horizon-browser-cli
   ```

   Result: The build finishes without an error.

2. Copy `horizon-browser` to `<run>/bin` and add its SHA-256 to `SHA256SUMS`.

   Result: `SHA256SUMS` contains the CLI.

3. Write a plan file below `<data-home>/smoke/bin`.

   ```json
   {"version":1,"steps":[{"id":"list","tool":"cloud_companions","arguments":{}}]}
   ```

   Result: The file contains one step.

4. In a fixture terminal in the workspace of `smoke-a`, run the plan.

   ```sh
   <run>/bin/horizon-browser run - < ~/smoke/bin/companions-plan.json
   ```

   Result: The output gives the same companion list as G07 step 2.

5. Add a step with `cloud_companion_ensure_ready` for `smoke-a` and `lib` to the plan.

   Result: The plan contains two steps.

6. Run the plan again.

   Result: The second step gives an `operation_id`. The companion stays Ready.

### 6.11 G11 — Compare the sibling checkout with the local commit

1. In `<sib>`, record the commit, the LFS files and the submodules.

   ```sh
   git -C <sib> rev-parse HEAD; git -C <sib> lfs ls-files; git -C <sib> submodule status
   ```

   Result: You have the local values. Save them in the private evidence.

2. In the worker shell of `smoke-sib`, show the same values for the sibling.

   ```sh
   cd ../sib && git rev-parse HEAD && git lfs ls-files && git submodule status
   ```

   Result: The commit, the LFS object IDs and the submodule commits are the same
   as in step 1.

3. Examine the LFS file in the sibling.

   ```sh
   git lfs ls-files -l | head -n 3
   ```

   Result: Each line shows `*`, so the file contains the content, not a pointer.

4. Show the status of the sibling checkout.

   ```sh
   git status --short
   ```

   Result: The output is empty.

### 6.12 G12 — Compare the companion cloud checkout and use it

1. In `<lib>`, record the commit, the LFS files and the submodules.

   ```sh
   git -C <lib> rev-parse HEAD; git -C <lib> lfs ls-files; git -C <lib> submodule status
   ```

   Result: You have the pinned commit and the content lists of the companion.

2. In the worker shell of `smoke-a`, show the commit of the companion worktree.

   ```sh
   ssh companion-lib git rev-parse HEAD
   ```

   Result: The commit is the same as in step 1.

3. Show the LFS files and the submodules of the companion worktree.

   ```sh
   ssh companion-lib 'git lfs ls-files; git submodule status'
   ```

   Result: The values are the same as the lists of step 1.

4. In the companion worktree, make a commit on a test branch with a synthetic author.

   ```sh
   ssh companion-lib 'git switch -c smoke-g12 && echo g12 > g12.txt && git add g12.txt && git -c user.name=Smoke -c user.email=smoke@example.invalid commit -m g12'
   ```

   Result: The command shows a new commit on `smoke-g12`.

5. In the worker shell of `smoke-a`, fetch the test branch through the alias.

   ```sh
   git fetch companion-lib:. smoke-g12:refs/smoke/g12 && git log -1 --oneline refs/smoke/g12
   ```

   Result: The output shows the commit `g12`. The source cloud can fetch from the companion.

6. Delete the test branch in the companion worktree.

   ```sh
   ssh companion-lib 'git switch - && git branch -D smoke-g12'
   ```

   Result: The test branch is gone. The companion worktree is clean.

## 7. Pass criteria

- The card of `smoke-a` shows `lib` and does not show the sibling `sib`.
- The sibling and the companion checkouts have the pinned commits, LFS content
  and submodule commits of the local checkouts.
- `ssh companion-lib` works when `lib` is Ready, and it fails after the clear.
- A clear while the target is stopped waits for the target. It completes after
  the resume.
- The MCP tools and the run plan start and stop the companion, and a repeated
  request starts no second worker.
- A companion cloud that never started gives `confirmation_required`.
- A Ready observation on the worker expires after 60 seconds while the candidate
  is paused.

## 8. Cleanup

1. Clear the checkbox of `lib` on the card of `smoke-a`.

   Result: The row shows **Not selected**.

2. In the worker shell of `smoke-a`, delete the test reference.

   ```sh
   git update-ref -d refs/smoke/g12
   ```

   Result: The reference is gone.

3. Make sure that the resource ledger contains `smoke-lib`, `smoke-lib0` and `smoke-sib`.

   Result: X01 deletes these clouds. Area L uses `smoke-sib`.

## 9. Record of results

Write the results in the report of the run. Use the
[report template](../../reports/TEMPLATE.md). Keep cloud IDs, grant IDs and
provider IDs out of the repository.
