---
procedure: new-cloud-fills-the-window
feature: New cloud dialog size
platforms: [linux, macos]
cost: none
destructive: no
secrets: [provider keys in Cloud settings]
owner: peters
---

# New cloud fills the window test procedure

## 1. Purpose

This procedure makes sure that the **New cloud** dialog takes the room of the
window it is open in, so that a small workspace does not force the person to
scroll a form the window could show.

## 2. Applicability

- Candidate: a build whose **New cloud** form is as wide as the window.
- Platforms: Linux and macOS.
- This procedure does not test: the offers, the profile chips and the start of a
  cloud. It starts no cloud.

## 3. Safety

> **CAUTION:** DO NOT CLICK **Start cloud**. This button rents compute from the
> provider. The provider charges money until somebody deletes the worker.

## 4. Equipment and preconditions

- One Horizon window of about 1480 by 900 pixels.
- One workspace that holds a panel, smaller than the canvas.
- Cloud settings that need no new key.

## 5. Setup

1. Note the width of the canvas and the width of the workspace that holds a
   panel.

   Result: The workspace is narrower than the canvas.

## 6. Tasks

### 6.1 WINDOW-1 — The form is as wide as the window

1. Press **+ New cloud**.

   Result: The form fills the window from the left margin to the right margin,
   and it is wider than the workspace behind it. **Summary** stands beside the
   fields, not below them.

2. Look for a scroll bar in the form.

   Result: The form shows its heading, its fields and its footer without a scroll
   bar while the content fits the window.

### 6.2 WINDOW-2 — The form follows a resize

1. Make the window about 1100 by 700 pixels.

   Result: The form shrinks with the window, keeps **Summary** beside the fields,
   and keeps its footer with **Cancel** and **Continue** visible.

2. Make the window about 1480 by 900 pixels again.

   Result: The form grows back and stays inside the window.

### 6.3 WINDOW-3 — A small window keeps the floor

1. Make the window about 700 pixels wide.

   Result: The form stops at its floor of 640 pixels, and it stays usable.

## 7. Pass criteria

- The form is as wide as the window, not as wide as the workspace that holds it.
- The two-column layout survives a resize, and the footer stays visible.
- The form never leaves the canvas, and it never goes below its floor.

## 8. Cleanup

1. Press **Cancel**.

   Result: The dialog closes and the workspace behind it is unchanged. No cloud
   was started.

## 9. Record of results

Put the results in the pull request.
