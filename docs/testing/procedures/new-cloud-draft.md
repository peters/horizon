---
procedure: new-cloud-draft
feature: The New cloud draft that a dismissal parks in its workspace
platforms: [linux]
cost: none
destructive: no
secrets: [the local login of the setup agent, for the optional task ND-6]
owner: peters
---

# New cloud draft test procedure

## 1. Purpose

This procedure proves that Escape and a click on the backdrop of **New cloud** put the dialog out of view and park
its draft in the workspace. It proves that the draft keeps every choice, that **Continue** opens the dialog again
and that **Cancel** ends the draft. It proves that a parked draft keeps its setup agent working.

## 2. Applicability

- Candidate: each candidate that changes **New cloud**, its dismissal or the draft card in a workspace.
- Platforms: Linux, in an isolated desktop with a live view. This procedure starts no worker, and it needs no
  provider credential. A provider key shows the worker cards of **Machine**; the catalog procedures test those.
- This procedure does not test: the start of a cloud, the proof of a setup agent and the commit of
  `.horizon`, or the Escape key of a cloud panel.
- Task ND-6 is optional. It needs a Linux computer with a systemd user session and a local agent login. Without
  them, the dialog shows the platform note and the task is skipped.

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

### 6.1 ND-1 — New cloud makes a workspace and opens its dialog
1. In the toolbar, click **+ New cloud**.
   Result: A new workspace appears on the board, and the **New cloud** dialog covers the board.
2. Examine the dialog.
   Result: It shows **Where is your code?** and **Cloud title**. Its footer shows **Continue** and **Cancel**.

### 6.2 ND-2 — Escape parks the draft and the workspace shows it
1. Type `<home>/smoke/atlas-app` in **Where is your code?**
2. Click **Continue**.
   Result: The dialog shows **Profile**, **Machine**, **Start cloud** and **Cancel**.
3. Type `Atlas cloud` in **Cloud title**.
4. Click `atlas-large` under **Profile**.
   Result: `atlas-large` is highlighted.
5. Press Escape.
   Result: The dialog is out of view, and the workspace shows a card.
6. Examine the card.
   Result: Line 1 is `Atlas cloud`, line 2 is `atlas-app · nothing rented yet`, line 3 is `Not started. Continue to finish the choices.` The card shows **Continue** and **Cancel**.

### 6.3 ND-3 — Continue and the backdrop keep every choice
1. Click **Continue** on the card.
   Result: The dialog opens again and covers the board. The card is out of view.
2. Examine **Cloud title**, **Where is your code?** and **Profile**.
   Result: The title, the repository and the profile `atlas-large` are the same as in ND-2.
3. Click the board outside the dialog.
   Result: The dialog is out of view, and the card is in the workspace again.
4. Click **Continue** on the card.
   Result: The dialog opens again, and every choice of step 2 is the same.

### 6.4 ND-4 — Cancel ends the draft and takes its empty workspace
1. Count the workspaces in the sidebar. Make a note of the count.
2. Press Escape.
   Result: The dialog is out of view, and the card is in the workspace.
3. Click **Cancel** on the card.
   Result: The card is out of view, and the count of the sidebar is one less.

### 6.5 ND-5 — Escape closes only the folder picker
1. In the toolbar, click **+ New cloud**.
   Result: A new workspace appears, and the dialog shows **Where is your code?**.
2. Type `https://github.com/peters/horizon.git` in **Where is your code?**
3. Click **Choose folder…**.
   Result: A picker with the heading **Select repository directory** covers the dialog.
4. Press Escape.
   Result: The picker is out of view. The dialog stays in view, and the link is still in the field.
5. Click **Cancel** in the footer of the dialog.
   Result: The dialog is out of view, and the count of the sidebar is one less.

### 6.6 ND-6 — A parked draft keeps its setup agent working (optional)
1. In the toolbar, click **+ New cloud**.
   Result: A new workspace appears, and the dialog shows **Where is your code?**.
2. Type `<home>/smoke/setup-app` in **Where is your code?**
3. Click **Continue**.
   Result: The dialog shows a card with the heading **Set up from CI** and the button **Set up with Claude**.
4. Click **Set up with Claude**.
   Result: The last line of the dialog says `Escape returns to the workspace. Show agent keeps Escape for an interrupt.` The footer shows **Close**.
5. Press Escape.
   Result: The dialog is out of view. The card says `Its setup agent is working; the agent's panel stays open`.
6. Open **Menu** › **Setup board**.
   Result: After 30 seconds the list shows one setup for `setup-app`, and the agent kept working.
7. Press Escape to close the setup board.
8. Click **Continue** on the card.
   Result: The dialog opens again and shows the progress of the same agent.
9. Click **Show agent**.
   Result: The panel of the agent opens with the keyboard focus in its terminal.
10. Press Escape in the terminal of the agent.
   Result: The terminal of the agent shows an interrupt, and the dialog stays in view.
11. Press Escape. Count the workspaces in the sidebar. Make a note of the count.
   Result: The dialog is out of view, and the card shows the setup status.

### 6.7 ND-7 — The toolbar and the menu reopen the parked draft
1. In the toolbar, click **+ New cloud**.
   Result: The dialog opens again with the same progress. The count is the same as in ND-6.
2. Press Escape.
   Result: The dialog is out of view, and the card shows the setup status.
3. Open **Menu** › **Cloud** › **Add cloud here**.
   Result: The dialog opens again with the same progress. One card shows, and the count is the same.

### 6.8 ND-8 — The board comes back as saved
1. Press Escape.
   Result: The card is in the workspace.
2. Click **Cancel** on the card.
   Result: The card is out of view, and the setup agent keeps working.
3. In the toolbar, click **+ New cloud**. Type `Saved draft` in **Cloud title**.
4. Press Escape.
   Result: The card is in the workspace, and its first line is `Saved draft`.
5. Close the window of the candidate with the close button of its window manager.
   Result: The process of the candidate exits.
6. Start the fixture again with the same state directory and `--native-view --saved-session`.
7. Open a Device panel with the new `vnc_address`.
   Result: The Device panel shows a live view of a new session.
8. Open **Menu** › **Sessions**. Click the oldest session in the list.
   Result: The board comes back with the workspace of the earlier start and no card, because Horizon does not save a draft.

## 7. Pass criteria

- Each task gives the result that its steps name, and no step starts a worker or a new provider resource.
- **Escape** and a click on the backdrop never lose a choice of the person.
- Only **Cancel** ends a draft, and it removes a workspace that held nothing.
- Examine these items in each screenshot and in the recording:
  - The card does not cover a cloud panel or a panel of its workspace.
  - The card moves and scales with its workspace when the board pans and zooms.
  - The accent border of the card is on all four sides, with no clipped edge.
  - The dialog opens again in the centre of the window, and no text of it is cut at 1600 × 1000.
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
