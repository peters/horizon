---
procedure: dependencies-panel
feature: Dependencies panel, setup steps and repository portfolio
platforms: [linux]
cost: none
destructive: no
secrets: [test worker SSH keys generated in a temporary folder]
owner: peters
---

# Dependencies panel test procedure

## 1. Purpose

This procedure proves that the Dependencies panel locks every step after
**Connect GitHub** until GitHub is connected, and that it shows a worker's
portfolio, instructions and diagnosis correctly.

## 2. Applicability

- Candidate: a build from the branch under test, frozen with its SHA-256.
- Platforms: Linux with X11 and a native Device panel to watch the candidate.
- This procedure does not test: a dependency worker in a cloud, real GitHub pull
  requests, CI, review or merges. Those are simulated by the test worker.

## 3. Safety

> **CAUTION:** USE ONLY THE TEST WORKER AND AN EPHEMERAL SESSION.
> Do not enter provider credentials. A normal session can rent compute.

> **CAUTION:** KEEP THE TEST WORKER FOLDER OUTSIDE THE REPOSITORY.
> It holds the generated private keys. Do not copy key values into evidence.

## 4. Equipment and preconditions

- Python 3 with `paramiko` and `PyYAML`, and OpenSSH.
- [The test worker](../../../scripts/dependencies-fixture/README.md).
- An isolated X11 display for the candidate, and a Device panel that shows it.
- No GitHub App in the candidate's Cloud settings for tasks DP-1 and DP-2.

## 5. Setup

1. Build the candidate and copy the binary to a new folder. Record its SHA-256.

   Result: The evidence names the exact binary.

2. Run the test worker tests.

   ```sh
   python3 scripts/dependencies-fixture/test_ssh.py
   python3 scripts/dependencies-fixture/test_safety.py
   ```

   Result: `test_ssh.py` prints `"passed": true`. `test_safety.py` prints `OK`.

## 6. Tasks

### 6.1 DP-1 — GitHub locks the setup

1. Start the candidate without `HORIZON_MAINTENANCE_FIXTURE`, with an ephemeral session.
2. Click **Dependencies** in the toolbar.

   Result: The panel shows **Set up Dependencies**, step 1 of 3. Step 1 shows
   **Not connected** and **Connect GitHub…**. Steps 2 and 3 show
   **Connect GitHub first**, and their buttons are disabled.

3. Click **Connect GitHub…**.

   Result: Cloud settings open. Close them without changes.

4. Click **Dependencies** again.

   Result: Horizon shows the same panel. It does not open a second panel.

### 6.2 DP-2 — No worker after GitHub

Do this task only with a GitHub App that the tester may connect. Otherwise
record DP-2 as not done.

1. Connect GitHub in Cloud settings.

   Result: Step 1 shows **Connected** and the app name. Step 2 offers
   **Choose on GitHub**. Step 3 says that this version cannot start a dependency
   worker, and **Start worker** stays disabled.

### 6.3 DP-3 — Test worker portfolio

1. Start the test worker in a new temporary folder.
2. Start the candidate with `HORIZON_MAINTENANCE_FIXTURE` set to that folder.
3. Open the Dependencies panel.

   Result: The portfolio opens with 21 repositories. The footer shows
   **Test worker** and says that GitHub, CI, review and merge are simulated.

4. Click the **Needs attention** tile.

   Result: Only the repositories that need attention show. Click the tile again.
   All 21 repositories show.

5. Type `nuget` in the search field.

   Result: Three repositories show. Clear the search with its clear button.

6. Click a repository with a blocked pull request.

   Result: The detail shows the status, each pull request and its GitHub link,
   the Dependabot updates and the instructions.

7. Scroll the mouse wheel over the detail.

   Result: The detail scrolls. The canvas does not move.

### 6.4 DP-4 — Instructions

1. Click **Instructions**. Change the global instructions. Click **Save to worker**.

   Result: The dialog closes. The footer shows **Instructions saved to the worker**.
   After the next status, the revision in the footer increases.

2. Stop the test worker. Open **Instructions**, change the text and save.

   Result: The dialog shows **Not saved** and keeps the draft. The health chip
   shows **Agent unknown · SSH unavailable**.

### 6.5 DP-5 — Diagnosis and terminal

1. Start the test worker again with the same folder.
2. Click **Debug with local agent**.

   Result: The dialog shows the agent state, process, heartbeat, instruction
   revisions, repositories and pull requests. It shows no prompts or keys.

3. Close the dialog. Click **Worker terminal**.

   Result: A terminal panel opens and follows the worker's run.

### 6.6 DP-6 — Themes and sizes

1. Repeat DP-3 steps 3 and 6 in the light theme and the dark theme.
2. Make the panel narrower than 980 points.

   Result: The selected repository's detail replaces the table. The close button
   returns to the table. No text overlaps.

## 7. Pass criteria

- DP-1: no step after GitHub unlocks without a GitHub App.
- DP-3 to DP-6 pass on the frozen candidate.
- The Device panel shows each task live. Screenshots alone are not evidence.

## 8. Cleanup

1. Close the candidate and stop the test worker.
2. Delete the test worker folder.

   Result: No generated key remains.
