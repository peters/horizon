---
procedure: cloud-agent-browser
feature: Browser tools of an agent panel in a cloud with agent isolation
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [provider API key in Cloud settings, Claude API key in Cloud settings]
owner: peters
---

# Cloud agent browser test procedure

## 1. Purpose

This procedure makes sure that the browser tools of an agent panel work in a
cloud with agent isolation. It also makes sure of these conditions:

- The control service runs as UID 10001, not as root.
- The cloud browsers run as UID 10001, not as root.
- The browser runtime root belongs to UID 10001.
- An agent can create a browser, navigate it and close it.

## 2. Applicability

- Candidate: a build that includes the fix for
  [issue #1307](https://github.com/peters/horizon/issues/1307). The worker image
  must come from the same commit.
- Platforms: Linux. Provider: Hetzner or RunPod.
- The stock worker image starts agent isolation in each cloud. A custom image
  without agent isolation runs agents and the control service as root.
- This procedure does not test these functions:
  - Remote browsers of a provider, for example BrowserStack.
  - Codex and Grok panels. They use the same browser tools as Claude Code.
  - A symbolic link at the browser runtime root. The unit tests in
    `examples/cloud-worker/test_browser_lane.py` cover it.

## 3. Safety

> **CAUTION:** DELETE THE CLOUD OF THIS RUN AT THE END. The provider charges
> money until somebody deletes the worker.

> **CAUTION:** DO NOT PUT AN API KEY IN A SCREENSHOT OR A LOG. A person who gets
> a key can use the account.

> **CAUTION:** OPEN ONLY PUBLIC TEST PAGES IN THE CLOUD BROWSER. Screenshots and
> recordings can show the content of each page.

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

### 6.1 B01 — Examine the account of the control service

1. Open the panel picker of the cloud.

   Result: The panel picker shows **Add panel**.

2. Click **Shell**.

   Result: A Shell panel opens and shows a prompt.

3. In the Shell panel, show the user of the shell.

   ```sh
   id -u
   ```

   Result: The output is `10001`.

4. Show the control service processes that UID 10001 runs.

   ```sh
   pgrep -u 10001 -f 'horizon-cloud-worker serve'
   ```

   Result: The output shows one process ID.

5. Show the control service processes that root runs.

   ```sh
   pgrep -u 0 -f 'horizon-cloud-worker serve'
   ```

   Result: The output is empty.

6. Show the owner and the mode of the browser runtime root.

   ```sh
   stat -c '%u %a' /workspace/home/.horizon
   ```

   Result: The output is `10001 700`.

7. Show the entries in the browser runtime root that UID 10001 does not own.

   ```sh
   find /workspace/home/.horizon ! -uid 10001
   ```

   Result: The output is empty.

### 6.2 B02 — Create a browser from the agent panel

1. Open the panel picker of the cloud.

   Result: The panel picker shows **Add panel**.

2. Click **Claude Code**.

   Result: A Claude Code panel opens and the agent starts.

3. Type this request to the agent.

   ```text
   Call browser_list. Then call browser_create with the URL https://example.com. Reply with the panel ID.
   ```

   Result: The agent replies with a panel ID that starts with `browser-`.
   Record it in the evidence as `<panel-id>`.

4. Examine the reply of the agent.

   Result: The reply does not show `denied access to a host coordination file`.

5. In the Shell panel, show the browser processes that UID 10001 runs.

   ```sh
   pgrep -u 10001 -c chrom
   ```

   Result: The output is a number that is more than 0.

6. Show the browser processes that root runs.

   ```sh
   pgrep -u 0 -c chrom
   ```

   Result: The output is `0`.

### 6.3 B03 — Navigate the browser

1. Type this request to the agent.

   ```text
   Call browser_navigate on <panel-id> with the URL https://example.org. Then call browser_snapshot and reply with the URL and the title.
   ```

   Result: The agent replies with the URL `https://example.org/`.

2. Examine the title in the reply.

   Result: The title is `Example Domain`.

### 6.4 B04 — Close the browser

1. Type this request to the agent.

   ```text
   Call browser_close on <panel-id>. Then call browser_list and reply with the number of panels.
   ```

   Result: The agent replies that `browser_list` shows 0 panels.

2. Wait 10 seconds.

3. In the Shell panel, show the browser processes that UID 10001 runs.

   ```sh
   pgrep -u 10001 -c chrom
   ```

   Result: The output is `0`.

### 6.5 B05 — Restart the cloud and create a browser again

1. On the card of the cloud, stop the worker.

   Result: The card shows that the worker stopped.

   > **CAUTION:** START ONLY THE WORKER OF THIS RUN. The provider charges money
   > again from the next step until the cleanup.

2. On the card of the cloud, start the worker again.

   Result: The card shows **Ready**.

3. Open a new Claude Code panel in the cloud.

   Result: The agent starts.

4. Type this request to the agent.

   ```text
   Call browser_create with the URL https://example.com. Then call browser_close on the new panel. Reply with the panel ID.
   ```

   Result: The agent replies with a panel ID that starts with `browser-`.

5. Open a new Shell panel in the cloud.

   Result: The Shell panel shows a prompt.

6. Show the control service processes that root runs.

   ```sh
   pgrep -u 0 -f 'horizon-cloud-worker serve'
   ```

   Result: The output is empty.

## 7. Pass criteria

- In B01, UID 10001 runs the control service and root does not.
- In B01, UID 10001 owns the browser runtime root and all of its entries.
- In B02, the agent gets a panel ID and no coordination file error.
- In B02, UID 10001 runs the cloud browser and root does not.
- In B03, the agent navigates the browser to `https://example.org/`.
- In B04, `browser_list` shows 0 panels and no browser process continues.
- In B05, the browser tools work after a restart of the worker.

## 8. Cleanup

> **CAUTION:** DELETE ONLY THE CLOUD OF THIS RUN. Its worker and its volume
> cannot come back.

1. On the card of the cloud, click **Delete cloud resources…**.

   Result: The card shows **Delete resources permanently**.

   > **CAUTION:** DELETE ONLY THE RESOURCES OF THIS CLOUD. The worker and the
   > volume cannot come back after the next step.

2. Click **Delete resources permanently**.

   Result: The card shows **Deleted**.

   > **CAUTION:** REMOVE ONLY THE CLOUD OF THIS RUN. Horizon removes the cloud
   > and its panels from the board.

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
