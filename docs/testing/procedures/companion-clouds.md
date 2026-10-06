---
procedure: companion-clouds
feature: Companion clouds, agent access to the SSH alias, the key and the catalog
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [provider API key in Cloud settings]
owner: peters
---

# Companion clouds test procedure

## 1. Purpose

This procedure makes sure that an agent session on a source cloud can use a
companion cloud. It also makes sure of these conditions:

- The agent user can read the catalog and use the SSH alias `companion-<alias>`.
- The agent user cannot read or change the root files of the companion.
- When you clear the selection, the agent user loses the alias and the key.

## 2. Applicability

- Candidate: a build that includes the fix for
  [issue #1315](https://github.com/peters/horizon/issues/1315).
- Worker image: an image from `examples/cloud-worker` at the same commit as the
  candidate. Both clouds use this image.
- Platforms: Linux host. Provider: Hetzner or RunPod.
- This procedure does not test:
  - A sibling on the same worker.
  - Start and stop of a companion from an agent. The MCP tools have unit tests.
  - Workers without agent isolation. Unit tests cover this case.

## 3. Safety

> **CAUTION:** RECORD THE CLOUD ID OF EACH CLOUD THAT YOU DEPLOY. Each worker
> costs money until you delete it.

> **CAUTION:** DO NOT PUT THE API KEY OR THE CONTENTS OF A PRIVATE KEY IN
> SCREENSHOTS, RECORDINGS OR LOGS. A person who gets them can use the account
> or the target worker.

> **CAUTION:** DELETE ONLY THE WORKERS THAT THIS RUN RECORDED. If you delete
> other workers, other people lose their work.

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256.
- The fixture `scripts/device-smoke/serve.py` with `--native-view`.
- A Device panel that shows a live view of the isolated desktop.
- A provider API key from the secret store of the test account.
- Two test repositories on GitHub with synthetic content:
  - The source repository. In this procedure, `example/source`.
  - The companion repository. In this procedure, `example/service`.
- A `.horizon/cloud.yml` file with a CPU profile in each repository.
- The source profile enables an agent, for example `claude`.

## 5. Setup

1. Start the fixture with a new directory for its state.

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <frozen-candidate> \
     --native-view --state <new-directory>
   ```

   Result: The fixture output shows a `vnc_address`.

2. Open a Device panel with the `vnc_address`.

   Result: The Device panel shows a live view of the candidate.

3. In the candidate, open **Cloud settings…**.

   Result: The Cloud settings dialog opens.

   > **CAUTION:** PASTE THE KEY ONLY INTO THE CANDIDATE IN THE ISOLATED DESKTOP.
   > If you paste it in another window, other people or logs can get the key.

4. Paste the provider API key.

   Result: The dialog shows that the key is saved.

## 6. Tasks

Give each task an ID. A report uses the ID to give a result.

### 6.1 K1 — Declare the companion

1. Add this block to `.horizon/cloud.yml` in the source repository.

   ```yaml
   companions:
     service:
       repository: example/service
       profile: cpu
   ```

   Result: The file declares the companion `service`.

2. Commit and push the change to the source repository.

   Result: The default branch of the source repository has the declaration.

### 6.2 K2 — Deploy the companion cloud

> **CAUTION:** DEPLOY ONLY ONE COMPANION CLOUD. The provider charges money for
> each worker until you delete it.

1. Open **Cloud › New cloud…** in the same workspace as the source cloud.

   Result: The New cloud dialog opens.

2. Type the path of a checkout of `example/service`.

   Result: The dialog reads the repository.

3. Select the CPU profile and the cheapest offer.

   Result: The summary shows the offer.

4. Click **Start cloud**.

   Result: The card of the companion cloud shows **Ready**.

5. Record the cloud ID of the companion cloud.

   Result: The run record has the cloud ID.

### 6.3 K3 — Deploy the source cloud

> **CAUTION:** DEPLOY ONLY ONE SOURCE CLOUD. The provider charges money for each
> worker until you delete it.

1. Open **Cloud › New cloud…** in the same workspace.

   Result: The New cloud dialog opens.

2. Type the path of a checkout of `example/source`.

   Result: The dialog reads the repository.

3. Select the CPU profile and the cheapest offer.

   Result: The summary shows the offer.

4. Click **Start cloud**.

   Result: The card of the source cloud shows **Ready**.

5. Record the cloud ID of the source cloud.

   Result: The run record has the cloud ID.

### 6.4 K4 — Select the companion

1. On the source card, open the **Connections** tab.

   Result: **Companion clouds** shows the checkbox `service` and
   `example/service · cpu`.

   > **CAUTION:** SELECT ONLY THE TEST COMPANION OF THIS RUN. Each agent session
   > on the source cloud gets shell access to the companion worker.

2. Select the checkbox `service`.

   Result: The row shows **Checking SSH access…**.

3. Wait until the row changes.

   Result: The row shows **Ready · SSH verified** and `ssh companion-service`.

### 6.5 K5 — Use the companion as the agent user

1. Add a **Shell** panel to the source cloud.

   Result: The panel shows a shell prompt on the source worker.

2. Run this command in the shell panel.

   ```sh
   id -un
   ```

   Result: The output is `horizon-agent`.

3. In **Companion clouds** on the source card, click **Refresh**.

   Result: The row shows **Ready · SSH verified**.

4. In the shell panel, run this command in the next 60 seconds.

   ```sh
   horizon-cloud-worker companions list
   ```

   Result: The JSON output has `"alias":"service"` and `"status":"ready"`.

   Note: A catalog that is older than 60 seconds shows `unverified`, not `ready`.

5. Record the `worktree` value in `access` from the output of step 4.

   Result: The value starts with `/workspace/companions/worktrees/`.

6. Run this command with the value from step 5.

   ```sh
   ssh companion-service 'git -C <worktree> log -1'
   ```

   Result: The output shows the newest commit of `example/service`.

7. Run this command.

   ```sh
   horizon-cloud-worker companions inspect service
   ```

   Result: The JSON output has `"status":"ready"`.

8. Run this command.

   ```sh
   ssh -G companion-service | grep -E '^(hostname|port) '
   ```

   Result: The output shows the address and the port of the companion worker.

9. Record the `hostname` and `port` values from step 8.

   Result: The run record has the address and the port.

   > **CAUTION:** DELETE THE KEY COPY IN TASK K8. The copy gives shell access
   > to the companion worker until the target revokes the grant.

10. Copy the key to a private file. This copy simulates a key that an agent kept.

    ```sh
    install -m 600 /run/horizon-companions/*/identity /workspace/home/companion-key-copy
    ```

    Result: The command shows no error.

### 6.6 K6 — Use the companion from an agent panel

1. Add an agent panel to the source cloud.

   Result: The agent starts in the panel.

2. In **Companion clouds** on the source card, click **Refresh**.

   Result: The row shows **Ready · SSH verified**.

3. In the next 60 seconds, tell the agent to call the MCP tool `cloud_companions_list`.

   Result: The tool result shows the alias `service` with the status `ready`.

4. Tell the agent to run `ssh companion-service 'git -C <worktree> log -1'`.

   Result: The output shows the same commit as task K5, step 6.

### 6.7 K7 — Root files stay private

Do these steps in the shell panel of task K5.

1. Run this command.

   ```sh
   ls /run/sshd/companions
   ```

   Result: The output shows `Permission denied`.

2. Run this command.

   ```sh
   cat /root/.ssh/config
   ```

   Result: The output shows `Permission denied`.

3. Run this command.

   ```sh
   ls -lnd /run/horizon-companions /run/horizon-companions/*/
   ```

   Result: Each directory has the owner `0`, the group `10001` and the mode `drwxr-x---`.

4. Run this command.

   ```sh
   ls -ln /run/horizon-companions /run/horizon-companions/*/
   ```

   Result: Each file has the owner `0`, the group `10001` and the mode `-rw-r-----`.

5. Run this command.

   ```sh
   touch /run/horizon-companions/config
   ```

   Result: The output shows `Permission denied`.

### 6.8 K8 — Clear the selection

> **CAUTION:** STOP ALL WORK ON THE COMPANION BEFORE YOU CLEAR THE CHECKBOX.
> The agent sessions lose access, and new SSH connections to the companion fail.

1. On the source card, clear the checkbox `service`.

   Result: The row shows **Removing access…**.

2. Wait until the row changes.

   Result: The row shows **Not selected**.

3. In the shell panel, run this command.

   ```sh
   ssh companion-service true
   ```

   Result: The output shows `Could not resolve hostname companion-service`.

4. Run this command.

   ```sh
   ls /run/horizon-companions
   ```

   Result: The output shows `catalog.json` and `config` only.

5. Run this command.

   ```sh
   horizon-cloud-worker companions list
   ```

   Result: The JSON output has `"selected":false` and `"access":null`.

6. Run this command with the values from task K5, step 9.

   ```sh
   ssh -F /dev/null -i /workspace/home/companion-key-copy -o IdentitiesOnly=yes \
     -o BatchMode=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
     -p <port> root@<hostname> true
   ```

   Result: The output shows `Permission denied (publickey)`.

   > **CAUTION:** DELETE ONLY `/workspace/home/companion-key-copy`. Other files
   > in `/workspace/home` hold the state of the agent sessions.

7. Delete the key copy.

   ```sh
   rm /workspace/home/companion-key-copy
   ```

   Result: The file `/workspace/home/companion-key-copy` does not exist.

## 7. Pass criteria

- In task K4, the row shows **Ready · SSH verified**.
- In tasks K5 and K6, the agent user reads the catalog and runs Git on the target.
- In task K7, the agent user cannot read the root files or change the copies.
- In task K8, the alias and the key copy are removed.
- In task K8, the target refuses the key that the agent copied.

## 8. Cleanup

1. Close the shell panel and the agent panel.

   Result: The panels close.

   > **CAUTION:** DELETE ONLY THE TWO CLOUDS THAT THIS RUN RECORDED. If you
   > delete other workers, other people lose their work.

2. On each recorded card, click **Delete cloud resources…** and accept the dialog.

   Result: The provider shows no worker with the recorded cloud IDs.

3. Revert the declaration commit in the source repository.

   Result: The source repository does not declare the companion.

4. Stop the fixture.

   Result: The fixture process stops and its Device panel shows no image.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
