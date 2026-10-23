---
procedure: cloud-feedback-visibility
feature: Why a cloud does not start, and its GitHub retry, stay readable
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [RunPod key in cloud settings, GitHub App sign-in of the test account if you do step 5]
owner: peters
---

# Cloud feedback visibility test procedure

## 1. Purpose

This procedure makes sure that a person sees why a cloud does not start, and
that a cloud's GitHub retry keeps a usable width. It makes sure that the reason
for a refused Start shows in the footer that holds the Start button. It also
makes sure that the reason appears once, and that a long reason cannot push the
dialog past the window.

## 2. Applicability

- Candidate: each candidate that changes the New cloud dialog footer, the cloud
  card's GitHub line, or the GitHub retry button.
- Platforms: Linux. The dialog and the card are the same on every platform, but
  this procedure was written and run on Linux.
- Lanes:
  - Lane D: the New cloud dialog shows a refused Start beside its buttons.
  - Lane G: a cloud card shows its refused GitHub sign-in above a full-width
    retry.
- This procedure does not test the deployment itself, pricing, or the worker.
  Use the cloud quick-start procedure for those.

## 3. Safety

> **CAUTION:** USE A PRIVATE HORIZON HOME AND A PRIVATE CONFIG. Do not run this
> procedure against your working Horizon state, because step 5 rents a worker.

> **CAUTION:** STEP 5 RENTS A WORKER. Stop and delete it at the end of step 6.
> The workspace volume stays billable after a stop.

> **CAUTION:** DO NOT SHOW A TOKEN, A KEY OR A SIGN-IN COOKIE IN THE EVIDENCE.
> Record only the dialog and the cloud card.

## 4. Equipment and preconditions

- A Linux computer with a working display, or an isolated desktop that you can
  view through a Device panel.
- A Horizon binary of the candidate, built with the default features.
- A private home and a private configuration:
  `HOME=<evidence>/home HORIZON_HOME=<evidence>/home/.horizon` with mode 700.
- A saved key for one cloud provider in
  `<evidence>/home/.horizon/cloud/settings.json`, for lane G. Hetzner is the
  cheapest provider for the quick start. Lane D needs no key.
- A small repository for lane D. A folder with a `.git` directory is enough. The
  procedure does not push to it.

## 5. Lane D: a refused Start shows its reason beside the Start button

1. Start the candidate with an ephemeral session, so that Start refuses a new
   cloud:

   ```bash
   HOME=<evidence>/home HORIZON_HOME=<evidence>/home/.horizon horizon --ephemeral
   ```

   Result: Horizon opens an empty board. The status line at the bottom says that
   the session is ephemeral.

2. Select **New cloud**.
3. In **Where is your code?**, type the path of the small repository, and press
   **Enter**.
4. Type a cloud title.
5. Select **Start cloud**.

   Result: the dialog stays open, and a red sentence shows in the footer, above
   the **Cancel** and **Start cloud** buttons. The sentence names the reason, for
   example that a saved session is necessary.

6. Read the footer.
   - The red sentence is on one or two lines directly above the buttons.
   - The sentence is not inside the column that holds the repository fields, and
     you do not have to scroll to read it.
   - The sentence shows one time. It does not repeat in the body of the dialog.
7. Shrink the window to about 800 by 600 pixels.
   - The whole dialog stays inside the window.
   - **Cancel** and **Start cloud** stay visible and are not clipped.
   - If your window manager does not resize the Horizon window, the automated
     check `creation_dialog_and_actions_fit_after_shrinking_with_scrolling_content`
     covers this step. It makes the dialog fit a small window with a reason that
     is 30 lines long.
8. Record a short video of steps 5 to 7 and a screenshot of step 6.

9. Optional: select a setup agent in the same dialog, so that the setup view
   replaces the Start button, and select **Start cloud** before the agent
   starts.

   Result: the red sentence shows above the setup agent's controls. It does not
   go away with the Start button.

## 6. Lane G: a refused GitHub sign-in shows a full-width retry

> **CAUTION:** THIS STEP RENTS A WORKER. Stop and delete the cloud in step 8.

1. Start the candidate without `--ephemeral`, with the same private home, so
   that a new cloud can start:

   ```bash
   HOME=<evidence>/home HORIZON_HOME=<evidence>/home/.horizon horizon
   ```

   Do not add `--ephemeral`. An ephemeral session refuses a new cloud.

2. Select **New cloud**. Choose the small repository, a title, and the quick
   start profile. Select **Start cloud**.
3. Wait until the cloud card asks for a GitHub sign-in.
4. Select **Skip: no GitHub for this cloud**.

   Result: the deployment continues, and the card records that the cloud has no
   GitHub access.

5. Wait until the cloud card reads **Ready**.
6. Open the card, and open the **Status** tab.
7. Read the GitHub line and its button.
   - The line starts with `GitHub:` and names why there is no access.
   - The button reads **Connect GitHub again** in one row, in normal button size.
   - The button is under the line, not beside it, and no letter is on its own
     row.
   - Select the button. It reacts, and it does not stay unresponsive. The card
     then asks for a GitHub sign-in again. Select **Skip: no GitHub for this
     cloud** to end that request.

   > **CAUTION:** SELECT THE BUTTON ONLY WHEN THE CARD READS **READY**. On a
   > stopped or deleted card the same click starts a new deployment. Stop and
   > delete that worker if it starts.
8. Stop the worker, then delete the cloud's resources. See the cloud
   quick-start procedure for the exact steps.
9. Record a short video of steps 6 and 7, and a screenshot of step 7.

## 7. Pass criteria

- Lane D: the reason for the refused Start is readable beside the buttons
  without scrolling, it shows once, and the dialog fits an 800 by 600 window.
- Lane G: the GitHub retry is a normal button in one row under its line, and it
  reacts to a click.
- No token, key or cookie shows in any evidence.
- The cloud from lane G is stopped and its resources are deleted.
