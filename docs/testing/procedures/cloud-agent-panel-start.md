---
procedure: cloud-agent-panel-start
feature: Agent panel start in a cloud, host instance and private browser state
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [provider API key in Cloud settings, Claude API key or subscription login]
owner: peters
---

# Cloud agent panel start test procedure

## 1. Purpose

This procedure makes sure that an agent panel in a cloud starts without a
permission error. It also makes sure that the agent gets the host instance and
cannot read the private browser state of the worker.

## 2. Applicability

- Candidate: a build that includes the fix for
  [issue #1306](https://github.com/peters/horizon/issues/1306). The worker image
  must come from the same commit.
- Platforms: Linux. Provider: Hetzner or RunPod.
- This procedure does not test: the browser tools of an agent.
  [Issue #1307](https://github.com/peters/horizon/issues/1307) tracks them.

## 3. Safety

> **CAUTION:** DELETE THE CLOUD OF THIS RUN AT THE END. The provider charges
> money until somebody deletes the worker.

> **CAUTION:** DO NOT PUT AN AGENT KEY OR A PROVIDER KEY IN A SCREENSHOT OR A
> LOG. A person who gets the key can use the account.

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256.
- The [local device smoke fixture](../../../scripts/device-smoke/README.md)
  with `--native-view`, and a Device panel that shows a live view.
- **Cloud settings…** with a provider key and a Claude key from the secret
  store of the test account.
- A repository with a `.horizon/cloud.yml` file. Its CPU profile selects the
  agent `claude` and the browser `chromium`.

## 5. Setup

> **CAUTION:** THE NEXT STEP RENTS COMPUTE. The provider charges money from
> this step until the cleanup.

1. Start a cloud with the CPU profile.

   Result: The card of the cloud shows **Ready**.

## 6. Tasks

### 6.1 H01 — Start a Claude Code panel

1. In the cloud, open the panel picker and click **Claude Code**.

   Result: A Claude Code panel opens and the agent starts.

2. Examine the first lines of the panel.

   Result: The panel shows no `Permission denied` line and no `host instance` line.

### 6.2 H02 — Examine the host instance

1. In the cloud, open the panel picker and click **Shell**.

   Result: A Shell panel opens and shows a prompt.

2. Show the user and the host instance of the Shell panel.

   ```sh
   id -u; printenv HORIZON_BROWSER_HOST_INSTANCE
   ```

   Result: The first line is `10001`. The second line is a UUID.

3. Show the published host instance, its owner and its mode.

   ```sh
   cat /run/horizon-worker/browser-host-instance
   stat -c '%U %a' /run/horizon-worker/browser-host-instance
   ```

   Result: The UUID is the same as in step 2. The owner is `root` and the mode is `644`.

### 6.3 H03 — Make sure that the browser state stays private

1. In the Shell panel, list the browser runtime root.

   ```sh
   ls /workspace/home/.horizon
   ```

   Result: The Shell panel shows `Permission denied`.

## 7. Pass criteria

- In H01, the Claude Code panel shows no `Permission denied` line.
- In H02, `HORIZON_BROWSER_HOST_INSTANCE` has the UUID of the published file.
- In H02, `root` owns the published file and its mode is `644`.
- In H03, the Shell panel cannot list the browser runtime root.

## 8. Cleanup

> **CAUTION:** THE NEXT STEP DELETES THE WORKER AND THE VOLUME OF THIS CLOUD.
> Their files cannot come back. Delete only the cloud of this run.

1. On the card of the cloud, click **Delete cloud resources…**.

   Result: The card shows **Delete resources permanently**.

2. Click **Delete resources permanently**.

   Result: The card shows **Deleted**.

3. Click **Remove cloud**.

   Result: The cloud is not on the board.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
