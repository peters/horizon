---
procedure: cloud-signin-window
feature: How long a cloud's browser sign-in waits, and the retry a parked cloud offers
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [GitHub App sign-in of the test account, Horizon cloud settings with a GitHub App]
owner: peters
---

# Cloud browser sign-in window test procedure

## 1. Purpose

This procedure proves that a cloud's browser sign-in waits fifteen minutes, that the
sign-in box states that window, that Skip ends the wait at once, and that a stopped or
deleted cloud offers no `Connect GitHub again`.

## 2. Applicability

- Candidate: builds with the shared `WEB_SIGNIN_WINDOW` of fifteen minutes.
- Platforms: Linux. W1, W2 and W4 need a configured provider account and a GitHub App.
- This procedure does not test: a first-time GitHub App creation, publishing images to
  ghcr.io, or GitHub access requests from an agent.

## 3. Safety

> **CAUTION:** W1, W2, W3 AND W4 RENT A WORKER. Stop the worker and delete its cloud
> resources when the lane ends. A running worker costs money for every minute it stays up.

> **CAUTION:** THE CLEANUP STEP DELETES THE TEST CLOUD'S WORKER AND STORAGE. Use a
> disposable test repository only. Do not delete a cloud that holds work you need.

Do not write a token, a password or a sign-in code in the report.

## 4. Equipment and preconditions

- A Linux computer with a display, and a build of the candidate.
- `x11vnc` for the view-only desktop, and a Horizon native VNC Device panel to watch it.
- A GitHub App in the Horizon cloud settings, and a disposable repository that the App
  is installed on.
- One configured compute provider account.

## 5. Setup

1. Start a private desktop for the test.

   Result: An empty Xvfb display and an `x11vnc` server on a loopback port.

2. Start the candidate with a private state directory, on that display.

   Result: The board opens, and `HORIZON_HOME` points at the private directory.

3. Open the native VNC Device panel on the loopback port.

   Result: The panel shows the board. `image_displayed` is true and `frame_sequence`
   increases.

## 6. Tasks

### 6.1 W1 — The sign-in box names the window

1. Select **New cloud**, enter the disposable repository, and select a small CPU worker.

   Result: The dialog shows the profile and the machine size.

   > **CAUTION:** THIS STEP RENTS A WORKER. The provider charges money from the next step
   > until the cleanup.

2. Select **Start cloud**.

   Result: The cloud card shows the steps. When the deployment reaches the GitHub step,
   the card shows **Connecting GitHub for this cloud** and the text that Horizon waits
   fifteen minutes and that Skip continues without GitHub.

3. Do not open the browser page.

   Result: The box keeps waiting. No retry button shows while the deployment runs.

### 6.2 W2 — Skip ends the wait at once

1. Select **Skip: no GitHub for this cloud**.

   Result: The box closes before fifteen minutes have passed, and the deployment reaches
   **Ready**.

2. Open the **Status** drawer.

   Result: One line reads `GitHub: Skipped: this cloud has no GitHub access.`, and
   `Connect GitHub again` shows at a readable width.

### 6.3 W3 — A parked cloud offers no retry

1. Select **Stop** on the cloud card and confirm.

   Result: The card shows the stopped state.

2. Open the **Status** drawer.

   Result: The GitHub line stays. **Connect GitHub again** does not show.

   > **CAUTION:** THIS STEP RENTS A WORKER AGAIN. The provider charges money from the next
   > step until the cleanup.

3. Start the cloud again with **Resume**, and open the drawer.

   Result: **Connect GitHub again** shows again only when the cloud is ready.

### 6.4 W4 — Run out of the window

> **CAUTION:** THIS LANE RENTS A SECOND WORKER. Stop every worker of the run and delete
> the cloud resources when the lane ends.

1. Start a second disposable cloud, and let the GitHub step open its sign-in box.

   Result: The box waits.

2. Do nothing for more than fifteen minutes.

   Result: The card reads `GitHub: GitHub did not finish the sign-in within 15 minutes.
   Connect GitHub again on the cloud card.`, and the retry shows. The line wraps, and
   `Connect GitHub again` keeps a readable width beside it or below it.

3. Select **Connect GitHub again**, and finish the sign-in in the browser.

   Result: The line changes to the account and its repositories. No second worker is
   started.

## 7. Cleanup

> **CAUTION:** THIS STEP DELETES THE TEST CLOUD'S WORKER AND STORAGE.

1. Stop each test cloud, then select **Manage ▸ Delete cloud resources…**.

   Result: The provider account shows no server and no volume for the test cloud.

2. Close the Device panel, stop `x11vnc`, and stop the private display.

   Result: No test process stays alive, and the developer's own Horizon stays untouched.

## 8. Pass criteria

- The sign-in box names fifteen minutes, and Skip ends the wait at once.
- The timeout line names fifteen minutes and offers the retry.
- A stopped or deleted cloud shows no `Connect GitHub again`.
- A retry on a ready cloud changes the GitHub line and starts no second worker.
