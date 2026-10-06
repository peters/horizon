---
procedure: cloud-agent-panel-start
feature: Agent panel start in a cloud, host instance and private browser state
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [provider API key in Cloud settings, Claude API key in Cloud settings]
owner: peters
---

# Cloud agent panel start test procedure

## 1. Purpose

This procedure makes sure that an agent panel in a cloud starts without a
permission error. It also makes sure of these conditions:

- The agent gets the host instance of the worker.
- The agent cannot read the browser runtime root of the worker.
- A new control service publishes a new host instance after a restart.

## 2. Applicability

- Candidate: a build that includes the fix for
  [issue #1306](https://github.com/peters/horizon/issues/1306). The worker image
  must come from the same commit.
- Platforms: Linux. Provider: Hetzner or RunPod.
- This procedure does not test these functions:
  - The browser tools of an agent.
    [Issue #1307](https://github.com/peters/horizon/issues/1307) tracks them.
  - Codex and Grok panels. They use the same launcher as Claude Code.

## 3. Safety

> **CAUTION:** DELETE THE CLOUD OF THIS RUN AT THE END. The provider charges
> money until somebody deletes the worker.

> **CAUTION:** DO NOT PUT THE CLAUDE API KEY OR THE PROVIDER API KEY IN A
> SCREENSHOT OR A LOG. A person who gets a key can use the account.

> **CAUTION:** USE ONLY SYNTHETIC CONTENT IN THE PANELS. Screenshots and
> recordings can show the content of each panel.

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256.
- The [local device smoke fixture](../../../scripts/device-smoke/README.md),
  `scripts/device-smoke/serve.py`, with `--native-view`.
- A provider API key and a Claude API key in the secret store of the test
  account.
- A synthetic repository with a `.horizon/cloud.yml` file. Its CPU profile
  selects the agent `claude` and the browser `chromium`.
- A private evidence directory, `<evidence>`, outside the fixture state.

## 5. Setup

1. Start the fixture with the frozen candidate and a new state directory.

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <frozen-candidate> \
     --native-view --state <new-directory>
   ```

   Result: The fixture output shows a `vnc_address`.

2. Open a Device panel with the `vnc_address`.

   Result: The Device panel shows a live view of the candidate.

3. In the candidate, open **Cloud settings…**.

   Result: The Cloud settings dialog opens.

   > **CAUTION:** PASTE EACH KEY ONLY INTO THE CANDIDATE IN THE ISOLATED
   > DESKTOP. If you paste it in another window, other people or logs can get
   > the key.

4. Paste the provider API key.

   Result: The dialog shows that the provider key is saved.

5. Paste the Claude API key.

   Result: The dialog shows that the Claude key is saved.

6. Click **Save settings**.

   Result: The dialog closes.

7. Open **Cloud › New cloud…**.

   Result: The New cloud dialog opens.

8. Type the path of the synthetic repository.

   Result: The dialog reads the repository.

9. Select the CPU profile.

   Result: The dialog shows the offers of the provider.

   > **CAUTION:** START ONLY ONE CLOUD FOR THIS RUN. The provider charges money
   > from the next step until the cleanup.

10. Click **Start cloud**.

    Result: The card of the cloud shows the deploy phases.

11. Wait until the card shows **Ready**.

    Result: The card shows **Ready**. Record the commit of the cloud in the evidence.

## 6. Tasks

Give each task an ID. A report uses the ID to give a result.

### 6.1 H01 — Start a Claude Code panel

1. Open the panel picker of the cloud.

   Result: The panel picker shows **Add panel**.

2. Click **Claude Code**.

   Result: A Claude Code panel opens and the agent starts.

3. Examine the first lines of the panel.

   Result: The panel shows no `Permission denied` line.

4. Examine the first lines of the panel again.

   Result: The panel shows no line that starts with `The worker has not published`.

5. Type a short request to the agent.

   ```text
   Reply with the word ready.
   ```

   Result: The agent replies `ready`.

### 6.2 H02 — Examine the host instance

1. Open the panel picker of the cloud.

   Result: The panel picker shows **Add panel**.

2. Click **Shell**.

   Result: A Shell panel opens and shows a prompt.

3. In the Shell panel, show the user of the shell.

   ```sh
   id -u
   ```

   Result: The output is `10001`.

4. Show the host instance of the Shell panel.

   ```sh
   printenv HORIZON_BROWSER_HOST_INSTANCE
   ```

   Result: The output is one UUID. Record it in the evidence as `<first-uuid>`.

5. Show the published host instance.

   ```sh
   cat /run/horizon-worker/browser-host-instance
   ```

   Result: The output is `<first-uuid>`.

6. Show the owner and the mode of the published file.

   ```sh
   stat -c '%U %a' /run/horizon-worker/browser-host-instance
   ```

   Result: The output is `root 644`.

### 6.3 H03 — Make sure that the browser state stays private

1. In the Shell panel, list the browser runtime root.

   ```sh
   ls /workspace/home/.horizon
   ```

   Result: The Shell panel shows `Permission denied`.

2. Try to change the published file.

   ```sh
   echo changed > /run/horizon-worker/browser-host-instance
   ```

   Result: The Shell panel shows `Permission denied`.

### 6.4 H04 — Restart the cloud and examine the new host instance

1. On the card of the cloud, stop the worker.

   Result: The card shows that the worker stopped.

2. On the card of the cloud, start the worker again.

   Result: The card shows **Ready**.

3. Open a new Shell panel in the cloud.

   Result: The Shell panel shows a prompt.

4. Show the host instance of the new Shell panel.

   ```sh
   printenv HORIZON_BROWSER_HOST_INSTANCE
   ```

   Result: The output is one UUID. It is not `<first-uuid>`.

5. Show the published host instance.

   ```sh
   cat /run/horizon-worker/browser-host-instance
   ```

   Result: The output is the UUID from step 4.

## 7. Pass criteria

- In H01, the Claude Code panel shows no `Permission denied` line.
- In H01, the agent replies `ready`.
- In H02, `HORIZON_BROWSER_HOST_INSTANCE` has the UUID of the published file.
- In H02, `root` owns the published file and its mode is `644`.
- In H03, the Shell panel cannot list the browser runtime root.
- In H03, the Shell panel cannot change the published file.
- In H04, the new Shell panel gets the new UUID, not `<first-uuid>`.

## 8. Cleanup

> **CAUTION:** DELETE ONLY THE CLOUD OF THIS RUN. Its worker and its volume
> cannot come back.

1. On the card of the cloud, click **Delete cloud resources…**.

   Result: The card shows **Delete resources permanently**.

2. Click **Delete resources permanently**.

   Result: The card shows **Deleted**.

3. Click **Remove cloud**.

   Result: The cloud is not on the board.

4. Close the Device panel of this run.

   Result: The Device panel closes. The fixture continues.

5. Stop the fixture with Ctrl-C.

   Result: The fixture removes its private home.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
