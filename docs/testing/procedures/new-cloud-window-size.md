---
procedure: new-cloud-window-size
feature: New cloud dialog size
platforms: [linux, macos]
cost: none
destructive: no
secrets: [provider keys in Cloud settings]
owner: peters
---

# New cloud window size test procedure

## 1. Purpose

This procedure makes sure that the **New cloud** dialog takes the room of the
window it is open in, up to the width its content can use. A small workspace must
not force the person to scroll a form that the window can show. A very wide
screen must not pull the fields and the **Summary** column apart.

## 2. Applicability

- Candidate: a build whose **New cloud** form takes its size from the window.
- Platforms: Linux and macOS.
- This procedure does not test: the offers, the profile chips and the start of a
  cloud. It starts no cloud.

## 3. Safety

> **CAUTION:** DO NOT CLICK **Start cloud**. This button rents compute from the
> provider. The provider charges money until somebody deletes the worker.

## 4. Equipment and preconditions

- One Horizon window of about 1480 by 900 pixels.
- One screen of more than 2000 pixels in width, for task 6.3. An ultra-wide
  screen is the best case.
- One workspace that holds a panel, smaller than the canvas.
- Cloud settings that need no new key.

## 5. Setup

1. Note the width of the canvas and the width of the workspace that holds a
   panel.

   Result: The workspace is narrower than the canvas.

## 6. Tasks

### 6.1 WINDOW-1 — The form takes the room of the window

1. Press **+ New cloud**.

   Result: The form is wider than the workspace behind it. **Summary** stands
   beside the fields, not below them.

2. Look for a scroll bar in the form.

   Result: The form shows its heading, its fields and its footer without a scroll
   bar while the content fits the window.

### 6.2 WINDOW-2 — The form follows a resize

1. Make the window about 1100 by 700 pixels.

   Result: The form shrinks with the window, keeps **Summary** beside the fields,
   and keeps its footer with **Cancel** and **Continue** visible.

2. Make the window about 1480 by 900 pixels again.

   Result: The form grows back and stays inside the window.

### 6.3 WINDOW-3 — A wide screen does not stretch the form

1. Move the Horizon window to the screen of more than 2000 pixels in width.

   Result: The window shows on that screen.

2. Make the window as wide and as tall as the screen allows.

   Result: The form stops at about 1180 pixels in width. It does not grow to the
   width of the window.

3. Look at the distance between the fields and the **Summary** column.

   Result: **Summary** stands next to the fields it summarizes, with one gutter
   between them. It does not stand at the other side of the screen.

4. Look at the height of the form.

   Result: The form keeps the height of the window. A full form uses every row,
   so the height has no limit of its own.

5. Type a path into **Where is your code?** and look at the field.

   Result: The field is as wide as the fields column, not as wide as the window.

### 6.4 WINDOW-4 — The narrowest window keeps the form usable

1. Make the window as narrow as Horizon allows.

   Result: The form is as wide as that window, and its two columns become one
   above the fields if the window is narrower than 800 pixels. Nothing is clipped
   and the footer stays visible.

## 7. Pass criteria

- The form takes its size from the window, not from the workspace that holds it.
- The form stops at about 1180 pixels in width, also on a much wider screen, and
  it keeps the height of the window.
- The two-column layout survives a resize, and the footer stays visible.
- The form never leaves the canvas, and it stays usable in the narrowest window.

## 8. Cleanup

1. Press **Cancel**.

   Result: The dialog closes and the workspace behind it is unchanged. No cloud
   was started.

## 9. Record of results

Put the results in the pull request.
