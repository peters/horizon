---
procedure: cloud-settings-keeps-its-form
feature: Cloud settings dialog dismissal
platforms: [linux, macos]
cost: none
destructive: no
secrets: [GitHub App secret in Cloud settings, provider keys in Cloud settings]
owner: peters
---

# Cloud settings keeps its form test procedure

## 1. Purpose

This procedure makes sure that a click on the backdrop of the **Cloud settings**
dialog does not close the dialog and does not discard what the person entered.
It also makes sure that **Cancel**, **Save settings** and Escape still close the
dialog.

## 2. Applicability

- Candidate: a build that keeps the **Cloud settings** dialog open when the
  person clicks outside it.
- Platforms: Linux and macOS.
- This procedure does not test: the values that **Save settings** writes, the
  Connect GitHub flow, and the other dialogs of Horizon.

## 3. Safety

No step rents compute, deletes data, sends a secret or changes access.

## 4. Equipment and preconditions

- One Horizon window, at least 1000 by 700 pixels.
- No cloud is needed. The procedure writes no settings when you cancel.

## 5. Setup

1. Open **Cloud settings** from the cloud panel.
2. Select the **Hetzner Cloud** key field and type `synthetic-typing-check`.

   Result: The field shows the text and the card still counts a change.

## 6. Tasks

### 6.1 CLOUD-SETTINGS-KEEP-1 — A click on the backdrop keeps the form

1. Click on the canvas outside the dialog, for example 40 pixels left of the
   dialog's left edge.

   Result: The dialog stays open. The typed text is still in the Hetzner key
   field.

2. Click on the dialog's own frame, for example on its heading.

   Result: The dialog stays open and the field keeps its text.

### 6.2 CLOUD-SETTINGS-KEEP-2 — Cancel and Escape still close it

1. Press **Escape**.

   Result: The dialog closes.

2. Open **Cloud settings** again and type `synthetic-typing-check` in the
   Hetzner key field.
3. Press **Cancel**.

   Result: The dialog closes and the field shows its saved value when you open
   the dialog again.

### 6.3 CLOUD-SETTINGS-KEEP-3 — Save still closes it

1. Open **Cloud settings** again.
2. Press **Save settings** without a change.

   Result: The dialog closes and no error is shown.

## 7. Pass criteria

- A click outside the dialog never closes it and never removes typed text.
- Escape, **Cancel** and **Save settings** each close the dialog exactly once.
- No other dialog of Horizon changes its behavior.

## 8. Cleanup

1. Confirm that the Hetzner key field shows its saved value, not
   `synthetic-typing-check`.

   Result: The synthetic text is gone. Nothing was saved.

## 9. Record of results

Put the results in the pull request. Keep no screenshots of real credentials.
