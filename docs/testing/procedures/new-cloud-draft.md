---
procedure: new-cloud-draft
feature: The New cloud draft that a parked surface leaves in its workspace
platforms: [linux]
cost: none
destructive: no
secrets: [the local login of the setup agent, for the optional task ND-6]
owner: peters
---

# New cloud draft test procedure

## 1. Purpose

This procedure proves that **New cloud** is a form on its workspace and not a dialog over the board. It proves
that a click outside the form and Escape change nothing. It proves that a draft parks in its workspace when that
workspace leaves the canvas, that the draft keeps every choice, that **Continue** shows the form again and that
**Cancel** ends the draft. It proves that a parked draft keeps its setup agent working. It proves that a parked
draft survives a quit and a session switch.

## 2. Applicability

- Candidate: each candidate that changes **New cloud**, its surface, its parking or the draft card in a workspace.
- Platforms: Linux, in an isolated desktop with a live view. This procedure starts no worker, and it needs no
  provider credential. A provider key shows the worker cards of **Machine**; the catalog procedures test those.
- This procedure does not test: the start of a cloud, the proof of a setup agent and the commit of
  `.horizon`, or the Escape key of a cloud panel.
- Task ND-6 is optional. It needs a Linux computer with a systemd user session and a local agent login. Without
  them, the form shows the platform note and the task is skipped.

## 3. Safety

> **CAUTION:** DO NOT CLICK **Start cloud**. This button rents compute from the provider.

> **CAUTION:** DO NOT SHOW AN AGENT LOGIN OR THE TEST REPOSITORY IN EVIDENCE. A screenshot can show a
> secret, and the setup agent reads the repository.

> **CAUTION:** DELETE ONLY THE FIXTURE STATE THAT THIS RUN MADE. Another run keeps its own sessions in the same
> parent directory.

## 4. Equipment and preconditions

- A Linux computer, a frozen candidate, the SHA-256 of the candidate, and a Device panel with a live view.
- The [local device smoke fixture](../../../scripts/device-smoke/README.md) with `--native-view` and `--saved-session`.
- A local login of Claude or Codex in the private home of the fixture, for the setup agent of the optional task ND-6.
  Docker on the host with about 10 GB of free disk makes its image build.
- A private evidence directory, `<evidence>`, outside the fixture state. The fixture home `<home>` is `<state>/data/home`.
- A synthetic repository `<home>/smoke/atlas-app` with one commit and this `.horizon/cloud.yml`. `<worker-image>` is a public CPU worker image, pinned by digest.

  ```yaml
  version: 1
  default: atlas-small
  profiles:
    atlas-small: {provider: runpod, image: <worker-image>, cpu: 2, memory_gb: 4}
    atlas-large: {provider: runpod, image: <worker-image>, cpu: 4, memory_gb: 8}
  ```

- A synthetic repository `<home>/smoke/setup-app` with one commit, one executable `test.sh` and `.github/workflows/ci.yml` with one job `test`. It has no `.horizon/cloud.yml`.

## 5. Setup

1. Start the fixture with the frozen candidate, a new state directory and `--native-view --saved-session`.
   Result: The fixture output shows a `vnc_address`.
2. Open a Device panel with the `vnc_address`.
   Result: The Device panel shows a live view. `frame_sequence` increases while the desktop changes.
3. Open **Menu** › **Sessions** › **Open New Session**.
   Result: The candidate shows a saved session.

4. Put the pointer on **+ New cloud** in the toolbar.
   Result: The button is not dim. Its hover text says "A new workspace with a cloud worker. Agents run there, also while this computer sleeps."

## 6. Tasks

### 6.1 ND-1 — New cloud makes a workspace and shows its form on it
1. In the toolbar, click **+ New cloud**.
   Result: A new workspace appears on the board. The **New cloud** form shows on that workspace, at its top-left
   corner. The board stays visible around the form, and the minimap does not show.
2. Examine the form.
   Result: It shows **Where is your code?** and **Cloud title**. Its footer shows **Continue** and **Cancel**. The
   worker catalog is on the left and the summary is on the right.
3. Drag an empty part of the canvas a short distance.
   Result: The workspace moves. The form stays below the name badge of the workspace, and it moves with it.
4. Examine the footer of the form.
   Result: No minimap or other overlay covers **Continue** or **Cancel**.

### 6.2 ND-2 — A workspace that leaves the canvas parks the draft
1. Type `<home>/smoke/atlas-app` in **Where is your code?**
2. Click **Continue**.
   Result: The form shows **Profile**, **Machine**, **Start cloud** and **Cancel**.
3. Type `Atlas cloud` in **Cloud title**.
4. Click `atlas-large` under **Profile**.
   Result: `atlas-large` is highlighted.
5. Drag an empty part of the canvas until the workspace is off the canvas.
   Result: The form is out of view. The view moves back to the workspace, and the workspace shows a card.
6. Examine the card.
   Result: Line 1 is `Atlas cloud`, line 2 is `atlas-app · nothing rented yet`, line 3 is `Not started. Continue to finish the choices.` The card shows **Continue** and **Cancel**.

### 6.3 ND-3 — A click outside the form and Escape keep every choice
1. Click **Continue** on the card.
   Result: The form shows again on its workspace. The card is out of view.
2. Examine **Cloud title**, **Where is your code?** and **Profile**.
   Result: The title, the repository and the profile `atlas-large` are the same as in ND-2.
3. Click an empty part of the canvas outside the form.
   Result: The form stays in view, and no card shows in the workspace.
4. Press Escape.
   Result: The form stays in view, and no card shows in the workspace.
5. Examine **Cloud title** and **Profile**.
   Result: The title and the profile `atlas-large` are the same as in step 2.

### 6.4 ND-4 — Cancel ends the draft and takes its empty workspace
1. Count the workspaces in the sidebar. Make a note of the count.
2. Drag an empty part of the canvas until the workspace of the form is off the canvas.
   Result: The form is out of view, and the card is in the workspace.
3. Click **Cancel** on the card.
   Result: The card is out of view, and the count of the sidebar is one less.

### 6.5 ND-5 — Escape closes only the folder picker
1. In the toolbar, click **+ New cloud**.
   Result: A new workspace appears, and the form shows **Where is your code?**.
2. Type `https://github.com/peters/horizon.git` in **Where is your code?**
3. Click **Choose folder…**.
   Result: A picker with the heading **Select repository directory** covers the form.
4. Press Escape.
   Result: The picker is out of view. The form stays in view, and the link is still in the field.
5. Click **Cancel** in the footer of the form.
   Result: The form is out of view, and the count of the sidebar is one less.

### 6.6 ND-6 — A parked draft keeps its setup agent working (optional)
1. In the toolbar, click **+ New cloud**.
   Result: A new workspace appears, and the form shows **Where is your code?**.
2. Type `<home>/smoke/setup-app` in **Where is your code?**
3. Click **Continue**.
   Result: The form shows a card with the heading **Set up from CI** and the button **Set up with Claude**.
4. Click **Set up with Claude**.
   Result: The last line of the form says `Close returns to the workspace. Show agent keeps Escape for an interrupt.` The footer shows **Close**.
5. Click **Close**.
   Result: The form is out of view. The card says `Its setup agent is working; the agent's panel stays open`.
6. Open **Menu** › **Setup board**.
   Result: After 30 seconds the list shows one setup for `setup-app`, and the agent kept working.
7. Press Escape to close the setup board.
8. Click **Continue** on the card.
   Result: The form shows again, with the progress of the same agent.
9. Click **Show agent**.
   Result: The panel of the agent opens with the keyboard focus in its terminal.
10. Press Escape in the terminal of the agent.
   Result: The terminal of the agent shows an interrupt, and the form stays in view.
11. Click **Close** in the footer of the setup view. Count the workspaces in the sidebar. Make a note of the count.
   Result: The form is out of view, and the card shows the setup status.

### 6.7 ND-7 — The toolbar and the menu reopen the parked draft
1. In the toolbar, click **+ New cloud**.
   Result: The form shows again with the same progress. The count is the same as in ND-6.
2. Click **Close**.
   Result: The form is out of view, and the card shows the setup status.
3. Open **Menu** › **Cloud** › **Add cloud here**.
   Result: The form shows again with the same progress. One card shows, and the count is the same.

### 6.8 ND-8 — A parked draft survives a session switch
1. In the toolbar, click **+ New cloud**. Type `Saved draft` in **Cloud title**. Drag an empty part of the
   canvas until the workspace of the form is off the canvas.
   Result: The card is in the workspace, and its first line is `Saved draft`.
2. Examine the file `<home>/.horizon/cloud/drafts.json`.
   Result: A moment after the park, the file holds one record for the workspace of the card. Its
   `title` is `Saved draft`, it names the session that holds the board, and it holds no token,
   check, clone or publishing choice.
3. Open **Menu** › **Sessions**. Click **Open New Session**.
   Result: The candidate shows the board of the new session. No card is in it.
4. Open **Menu** › **Sessions**. Click the session that holds the card, then **Switch Selected**.
   Result: The board of that session comes back, and the card shows in its workspace. Its first line
   is `Saved draft`.
5. Open **Menu** › **Sessions**. Click the new session of step 3, then **Switch Selected**.
   Result: The board of the new session holds no card of `Saved draft`. Examine `drafts.json`:
   the record of the card is still there.
6. Repeat step 4.
   Result: The card shows again in its workspace, and its first line is `Saved draft`.
7. Click **Continue** on the card.
   Result: The form is in view, and **Cloud title** shows `Saved draft`.
8. Drag an empty part of the canvas until the workspace of the form is off the canvas, then click **Cancel** on the
   card.
   Result: The card is out of view. Examine `drafts.json` again: the record of that workspace is gone.

### 6.9 ND-9 — A workspace that leaves the board keeps the form reachable
1. In the toolbar, click **+ New cloud**. Type `Moved draft` in **Cloud title**.
2. Move the workspace of the form to its own window: right-click the workspace in the sidebar, then click
   **Detach**, or use the workspace menu of the title bar.
   Result: The form is out of view on the board.
3. In the toolbar, click **+ New cloud**.
   Result: The form shows on the board, not on a workspace, and **Cloud title** shows `Moved draft`.
4. Click **Cancel**.
   Result: The form is out of view, and the count of the sidebar is one less.

Note: the fixture removes its private home when it stops, so a run cannot restart it in the same
state. A quit uses the same saved record and the same restore path as the session switch of step 4;
the automated test that restores a parked draft into a fresh app covers it.

## 7. Pass criteria

- Each task gives the result that its steps name, and no step starts a worker or a new provider resource.
- A click outside the form, and Escape, never lose a choice of the person and never park the draft.
- Only **Cancel** ends a draft, and it removes a workspace that held nothing.
- A draft whose workspace moved to its own window or left the board shows its form on the board, and
  **Cancel** stays reachable.
- A session switch ends the card that is in the form, also a card that a record brought back, and
  the record waits for the session that owns it.
- The card of a parked draft comes back after a session switch and after a quit, and it keeps the title and the
  repository of the draft.
- Examine these items in each screenshot and in the recording:
  - The card does not cover a cloud panel or a panel of its workspace.
  - The card moves and scales with its workspace when the board pans and zooms.
  - The accent border of the card is on all four sides, with no clipped edge.
  - The form opens again on its workspace, no text of it is cut at 1600 × 1000, and no overlay covers its
    footer.
  - The card text is readable in the dark palette and in the light palette of **Settings**.

## 8. Cleanup

1. Open **Menu** › **Setup board**.

   > **CAUTION:** END ONLY THE SETUP RUN THAT THIS RUN STARTED. A run of another folder can commit work that a person did not save.

2. End the setup run of `setup-app`.
   Result: The list is empty.
3. Close the window of the candidate with the close button of its window manager.
   Result: The process of the candidate exits.
4. Stop the fixture with Ctrl+C in its terminal.
   Result: The display and the Device panel of the run stop.
5. Remove the state directory, the frozen candidate and `<evidence>`.
   Result: The paths of this run are gone.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with [the report template](../reports/TEMPLATE.md). If not, put the results in the pull request. Keep private evidence out of the repository.
