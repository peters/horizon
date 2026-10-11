---
procedure: cloud-first-use-smoke
feature: Cloud setup and first-use repair
platforms: [linux, macos, windows]
cost: rents compute
destructive: yes
secrets: [approved compute key, approved agent credentials, task registry bindings]
owner: peters
---

# Cloud first-use test procedure

## 1. Purpose

This procedure tests Cloud setup, missing-account repair, authentication choices, cancellation and saved settings.
It replaces [the old plan](../../archive/cloud-first-use-smoke-plan.md).
Use [the Cloud panels procedure](cloud-panels.md) for the full worker and panel matrix.

## 2. Applicability

- Candidate: the exact PR head with the Cloud setup forms.
- Platforms: Linux, macOS and Windows, as approved for the run.
- Local form tests use synthetic credentials and allocate no compute.
- Worker tests need separate approval for the provider, quota, repository and test lanes.
- Worker readiness does not prove agent authentication or application readiness.

## 3. Safety

> **CAUTION:** USE ONLY THE APPROVED PROVIDER AND QUOTA. Worker tests rent compute.

> **CAUTION:** USE A PRIVATE TEST HOME. Tests must not replace active settings or expose credentials.

> **CAUTION:** STOP ONLY TASK-OWNED RESOURCES. Other workspaces, workers and sessions must remain intact.

## 4. Equipment and preconditions

- The frozen candidate, its commit and its binary hash.
- A task-owned desktop with a public Horizon native Device panel.
- A private home, settings directory and session for the candidate.
- Synthetic provider and agent values for local form tests.
- A committed fixture repository with `.horizon/cloud.yml`.
- A second fixture repository without that file.
- Approved secret references and compute quota for each worker test.
- The full repository validation matrix and an independent review.

## 5. Setup

1. Start the frozen candidate with the private home and an ephemeral session.

   Result: The candidate has no inherited cloud settings or agent credentials.

2. Examine the application child PID and binary hash.

   Result: Both identify the frozen candidate.

3. Open the native Device panel for the task-owned desktop.

   Result: The public panel reports a connected image with displayed advancing frames.

4. Record three public panel inspections at least two seconds apart.

   Result: The observations prove live presentation of changing target output.

5. Start a native desktop recording with synthetic data only.

   Result: The recording belongs to this exact candidate and desktop.

## 6. Tasks

### 6.1 F01 — First use and repair

1. Open **Cloud > New cloud** with no saved compute key.

   Result: The repair form opens. The submitted title and workspace remain selected.

2. Click **Cancel**.

   Result: No cloud or worker is created. Focus returns to the previous view.

3. Repeat the flow with **Escape**.

   Result: The form closes without allocation.

4. Open **Cloud > Cloud settings**.

   Result: The form shows compute access, coding agents, registry bindings and workspace defaults.

### 6.2 F02 — Agent choices and private save

1. Enter a synthetic compute key.

   Result: The field masks the value.

2. Select **Codex** and **Claude**.

   Result: Codex offers **API key** and **ChatGPT plan**. Claude offers **API key** and **Subscription login**.

3. Select **API key** for Codex and **Subscription login** for Claude.

   Result: Only the selected API-key mode requires a saved key.

4. Click **Save settings** with the Codex key empty.

   Result: The error names the Codex API key and ChatGPT plan choices. Existing settings remain unchanged.

5. Enter a synthetic Codex API key and save the settings.

   Result: Private files contain the bindings. Logs, configuration YAML and panel state contain no secret.

6. Reopen the form and save blank replacement fields.

   Result: The saved bindings remain unchanged.

7. Test Claude in API-key mode with its key empty.

   Result: The error names the Claude API key and subscription login choices.

> **CAUTION:** USE ONLY THE APPROVED TEST ACCOUNT. This task changes account access and stores private credentials. Do not record real credentials.

8. Run [the local plan sign-in procedure](sign-in-with-chatgpt.md) when the lane needs **ChatGPT plan**.

   Result: The local grant satisfies form Save. Worker terminal authentication remains separate.

9. Test a worker login that needs a browser on another device.

   Result: The instructions identify the required device and browser steps. They do not claim that local plan sign-in authenticates the worker.

### 6.3 F03 — Repository setup and Create cancellation

1. Select the committed fixture repository and profile.

   Result: Create records the selected profile and committed revision without allocating compute.

2. Select the fixture repository without `.horizon/cloud.yml`.

   Result: The form offers a setup agent and **Reload configuration**.

3. Open the setup agent from an existing cloud workspace.

   Result: The agent opens in a compatible ordinary workspace. It allocates no worker and inherits no remote credential.

> **CAUTION:** REPLACE ONLY THE TASK-OWNED FIXTURE. This step changes the test configuration. Keep the original fixture for recovery.

4. Replace the fixture configuration with malformed YAML and reload it.

   Result: Stale profiles clear. Create remains disabled until a valid configuration loads.

5. Cancel a slow repository check.

   Result: The form remains responsive. No late result creates a cloud. The task-owned check stops within its limit.

6. Retry while the cancelled check stops.

   Result: At most one check runs. A pending stop gives a retryable explanation.

> **CAUTION:** REMOVE ONLY A TASK-OWNED WORKSPACE. This step deletes test workspace metadata. Keep unrelated workspaces intact.

7. Switch sessions or remove the captured workspace before the check completes.

   Result: No late result creates a cloud in another session or workspace.

8. Prepare a profile with an already pinned commit. Repeat with an invalid or unresolved revision.

   Result: Preparation does not inspect Git or the repository mount on the UI thread. Invalid revisions are rejected. Background deployment validates committed source before allocation.

### 6.4 F04 — Approved worker lane

> **CAUTION:** USE ONLY THE APPROVED PROVIDER AND QUOTA. This step rents compute. Stop when the approved cost or time limit is reached.

1. Run the approved deployment lanes in [the Cloud panels procedure](cloud-panels.md).

   Result: Only the approved compute quota is used. Deployment uses committed source.

2. Examine the measured stage activity and time.

   Result: Transfer speed and ETA appear only when measurable. Worker readiness has its own measured time.

> **CAUTION:** USE ONLY APPROVED TEST CREDENTIALS. This step changes worker access. Keep credentials out of logs and recordings.

3. Complete the actual worker CLI authentication for each selected subscription agent.

   Result: The real worker session authenticates. Local plan status alone does not prove worker authentication.

### 6.5 F05 — Layout and persistence

1. Resize the candidate to narrow and 4K views.

   Result: The toolbar overflow preserves Cloud access. Forms scroll and retain accessible footer actions.

2. Test fit, nested fullscreen and **Escape**.

   Result: No controls overlap. The correct view closes or restores.

3. Restart only the task-owned candidate.

   Result: Saved bindings, modes, cloud membership and worker identities survive.

4. Repeat cancellation and reconnect lanes.

   Result: No duplicate worker is allocated.

> **CAUTION:** REMOVE ONLY THE TASK-OWNED TEST CLOUD. This step deletes saved test metadata. Make sure the selected cloud belongs to the test.

5. Remove the last initialized test cloud and restore the session.

   Result: The saved empty cloud list remains empty.

6. Close before the first cloud preparation frame. Repeat immediately after a session switch.

   Result: Existing saved groups survive both restarts.

### 6.6 F06 — Compatibility and ownership

1. Run the full repository validation matrix, including the no-default-features core tests and worker Python suite.

   Result: Required tests pass. Worker context inputs match the allowlist and exclude credentials and backups.

2. Restore a cloud session in a build without cloud support.

   Result: Autosave preserves opaque metadata and panel identities. Inert cloud commands never run locally.

3. Attempt an ordinary panel move into a cloud through the sidebar and minimap.

   Result: Membership and layout remain unchanged. Compatible ordinary moves still work.

> **CAUTION:** CHANGE ONLY TASK-OWNED CREDENTIALS. These lanes replace or revoke access. Keep unrelated accounts and bindings intact.

4. Run the credential-generation and ownership lanes in [the Cloud panels procedure](cloud-panels.md).

   Result: Replaced credentials require fresh catalog rows. Late old-generation results cannot restore stale access.

5. Run the focused Device resize-journal tests.

   Result: A pending journal without an observation returns `resize_uncertain` and preserves prior ownership.

6. Hide the task-owned native viewer while target output changes.

   Result: Reception continues. Public observations distinguish received frames from displayed frames.

7. Return to the viewer through the UI.

   Result: The displayed image and advancing frames prove live presentation.

8. Run the directory-durability and Unix session synchronization tests.

   Result: Unsupported durability fails before state writes or provider mutations. Missing saved files cause failure.

9. Load legacy settings and profiles. Test disabled agents and browser capabilities, multiple task-owned instances and remote browser settings.

   Result: Supported legacy values load correctly. Disabled capabilities remain disabled. Each instance preserves its own settings. Remote browser settings retain their configured values.

> **CAUTION:** REMOVE ONLY A TASK-OWNED WORKSPACE. This step deletes test workspace metadata. Keep unrelated workspaces intact.

10. Remove an ordinary workspace when another compatible ordinary workspace exists. Repeat when only cloud destinations remain.

    Result: The compatible ordinary workspace is selected. If no compatible destination exists, removal is refused. No panel is orphaned.

## 7. Pass criteria

- Each applicable task passes on the exact candidate.
- Credentials remain private and masked.
- Cancellation creates no late cloud or duplicate worker.
- Settings survive private saves and task-owned restarts.
- The decoded recording shows the applicable visual states.
- Required local checks, independent review and exact-head hosted checks pass.
- A blocked or unapproved paid lane has a separate result and no success claim.

## 8. Cleanup

> **CAUTION:** REMOVE ONLY THE EXACT TASK RESOURCES. This step deletes compute and its data. Make sure the resources belong to the test.

1. Release task devices before you remove approved test compute.

   Result: The provider reports the exact task resources absent.

> **CAUTION:** REVOKE ONLY TASK REGISTRY BINDINGS. This step removes credential access. Keep unrelated bindings intact.

2. Revoke task registry bindings.

   Result: Task credentials no longer grant access.

3. Close the exact candidate through its normal window-manager path.

   Result: The candidate and its owned desktop processes exit.

> **CAUTION:** DELETE ONLY THE PRIVATE TEST STATE. This step deletes files. Make sure the path belongs to the test and its processes stopped.

4. Remove the private test state after the owned processes exit.

   Result: Active settings and unrelated resources remain intact.

## 9. Record of results

Use [the report template](../reports/TEMPLATE.md) for retained runs.
Otherwise, put the results in the PR. Identify the exact candidate and each applicable task.
Keep private credentials, host paths and account data out of published evidence.
