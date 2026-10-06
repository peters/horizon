---
procedure: cloud-stopped-panel-restore
feature: Restored panels of a cloud that is stopped or that reconnects
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [provider API key in Cloud settings, Claude API key in Cloud settings]
owner: peters
---

# Cloud stopped panel restore test procedure

## 1. Purpose

This procedure makes sure that a restored panel of a cloud tells why it does not
run. When the cloud is stopped, the panel tells you that the cloud is stopped and
that **Resume worker** restores the panel. When Horizon reconnects the cloud, the
panel tells you that Horizon reconnects it. The panel does not tell you to fix the
command or the binary.

## 2. Applicability

- Candidate: a build that includes the fix for
  [issue #1324](https://github.com/peters/horizon/issues/1324).
- Platforms: Linux. Provider: Hetzner or RunPod. Do the procedure on one
  provider. The other provider is optional.
- This procedure does not test these functions:
  - The idle stop. Task S01 uses **Stop worker…**, which gives the same
    stopped cloud.
  - A panel whose command or binary is not correct. That panel keeps the text
    "Fix the command or binary, then restart the panel."

## 3. Safety

> **CAUTION:** DELETE THE CLOUD OF THIS RUN AT THE END. The provider charges
> money until somebody deletes the worker and its storage.

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
  selects the agent `claude`.
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

4. Paste the provider API key and the Claude API key. Then click **Save settings**.

   Result: The dialog closes.

5. Open **Cloud › New cloud…**. Type the path of the synthetic repository and
   select the CPU profile.

   Result: The dialog shows the offers of the provider.

   > **CAUTION:** START ONLY ONE CLOUD FOR THIS RUN. The provider charges money
   > from the next step until the cleanup.

6. Click **Start cloud**. Wait until the card shows **Ready**.

   Result: The card shows **Ready**.

7. Open the panel picker of the cloud. Click **Claude Code**.

   Result: A Claude Code panel opens and the agent starts.

## 6. Tasks

Give each task an ID. A report uses the ID to give a result.

### 6.1 S01 — Restore a panel of a stopped cloud

1. On the card, click **Stop worker…**. Then confirm the stop.

   Result: The card shows **Stopped** and **Resume worker**.

2. Close the candidate window with the window manager. Start the candidate again
   with the same state directory.

   Result: The board shows the cloud and its Claude Code panel.

3. Examine the Claude Code panel.

   Result: The panel shows these lines:

   ```text
   The cloud of this panel is stopped.

   Panel: <name of the panel>

   Choose Resume worker on the cloud card to restore this panel.
   ```

   The panel does not show "Fix the command or binary". Record a screenshot in
   the evidence.

4. Examine the card.

   Result: The card shows **Stopped** and **Resume worker**.

### 6.2 S02 — Resume the worker

> **CAUTION:** THE NEXT STEP STARTS THE WORKER AGAIN. The provider charges money
> for the compute until the cleanup.

1. On the card, click **Resume worker**.

   Result: The panel shows these lines until the cloud is ready:

   ```text
   Horizon is reconnecting the cloud of this panel.
   ```

2. Wait until the card shows **Ready**.

   Result: The Claude Code panel shows the agent again. It does not show the
   stopped text.

### 6.3 S03 — Restore a panel of a ready cloud

1. Close the candidate window with the window manager. Start the candidate again
   with the same state directory.

   Result: While the card shows the reconnect, the panel shows
   "Horizon is reconnecting the cloud of this panel." and
   "The panel comes back when the cloud is ready."

2. Wait until the card shows **Ready**.

   Result: The Claude Code panel shows the agent again.

## 7. Pass criteria

- In S01, the panel names the stopped cloud and **Resume worker**.
- In S01, S02 and S03, no panel of the cloud shows "Fix the command or binary".
- In S02 and S03, the panel shows the agent when the card shows **Ready**.

## 8. Cleanup

> **CAUTION:** THE NEXT STEP DELETES THE WORKER AND ITS STORAGE. Do it only on
> the cloud of this run.

1. On the card, open **Manage**. Click **Delete cloud resources…**. Then click
   **Delete resources permanently**.

   Result: The card shows that the cloud is deleted.

2. Close the candidate window. Stop the fixture.

   Result: No candidate process from this run continues.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
