---
procedure: new-cloud-fills-an-empty-workspace
feature: New cloud and the workspace it opens in
platforms: [linux, macos]
cost: none
destructive: yes
secrets: [provider keys in Cloud settings]
owner: peters
---

# New cloud fills an empty workspace test procedure

## 1. Purpose

This procedure makes sure that **+ New cloud** opens its dialog in an empty
workspace that the board already has, instead of making another one, and that a
cancelled draft never removes a workspace the person made.

## 2. Applicability

- Candidate: a build in which **+ New cloud** fills an empty workspace.
- Platforms: Linux and macOS.
- This procedure does not test: the offers, the profile chips and the start of a
  cloud. It starts no cloud.

## 3. Safety

> **CAUTION:** DO NOT CLICK **Start cloud**. This button rents compute from the
> provider. The provider charges money until somebody deletes the worker.

## 4. Equipment and preconditions

- One Horizon window with one workspace that holds at least one terminal panel.
- Cloud settings that need no new key, so the dialog opens.

## 5. Setup

1. Read the count beside **This PC** in the sidebar.

   Result: The count shows the number of workspaces, for example `1`.

## 6. Tasks

### 6.1 FILLS-EMPTY-1 — An empty workspace is filled, not doubled

1. Press **New** beside **Workspaces**.

   Result: The count is one higher, and the new workspace is empty.

2. Press **+ New cloud**.

   Result: **New cloud** opens in the empty workspace. The count does not change,
   and no third workspace appears.

### 6.2 FILLS-EMPTY-2 — Cancel keeps the workspace the person made

1. Press **Cancel**.

   Result: The dialog closes, the workspace and its row stay, and the count is
   the same as in step 1 of 6.1.

### 6.3 FILLS-EMPTY-3 — A board with no empty workspace still gets one

1. Open a terminal in the empty workspace, so that no workspace is empty.
2. Press **+ New cloud**.

   Result: The count is one higher, so the cloud took a new workspace.

3. Press **Cancel**.

   Result: The count goes back, because that workspace was made for the draft.

## 7. Pass criteria

- **+ New cloud** fills an empty workspace and makes no new one.
- **Cancel** keeps a workspace the person made, and removes only the workspace a
  draft made for itself.
- A workspace that holds a panel is never taken for a new cloud.

## 8. Cleanup

1. Confirm that the workspace that holds the terminal panel still runs.

   Result: Nothing else changed. No cloud was started.

## 9. Record of results

Put the results in the pull request.
