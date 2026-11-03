---
procedure: cloud-github-sign-in-mode
feature: Cloud settings, the GitHub sign-in of each mode
platforms: [linux, macos]
cost: none
destructive: no
secrets: [GitHub App secret in Cloud settings]
owner: peters
---

# Cloud GitHub sign-in mode test procedure

## 1. Purpose

This procedure makes sure that the GitHub card tells each mode how it signs in:
**Automatic** signs in through the browser and never asks for GitHub's device-flow
setting, and **Ask me for each new cloud** reports that setting.

## 2. Applicability

- Candidate: a build whose GitHub card names the sign-in of its mode.
- Platforms: Linux and macOS.
- This procedure does not test: creating the app, the repositories, and a cloud's
  own sign-in.

## 3. Safety

No step rents compute, deletes data or changes access.

## 4. Equipment and preconditions

- One Horizon window with a connected GitHub app.
- Cloud settings that open.

## 5. Setup

1. Open **Cloud settings** and read the GitHub card.

   Result: The card shows the app and its mode.

## 6. Tasks

### 6.1 SIGN-IN-MODE-1 — Automatic names the browser

1. Select **Automatic (no clicks after the first approval)**.

   Result: A new cloud starts in Automatic. The card says **Sign-in runs in the
   browser Horizon opens.**

2. Read the card again after a few seconds.

   Result: The card never says **Device sign-in is on.**, and it never asks you to
   turn on **Enable Device Flow** in the app's settings.

### 6.2 SIGN-IN-MODE-2 — Ask reports the device flow

1. Select **Ask me for each new cloud (one Authorize click)**.

   Result: The card reports the device-flow state of the app: **Device sign-in is
   on.**, or **Turn on Enable Device Flow in the app's settings, once.** with
   **Open app settings** and **Check again**.

2. Press **Open app settings** when the card asks for it.

   Result: The app's settings page opens, and **Check again** reports the state
   after you change it.

### 6.3 SIGN-IN-MODE-3 — Both modes offer the sign-in of this computer

1. In each mode, read the line under the mode of the sign-in.

   Result: With no sign-in on this computer, the card says so in yellow and
   offers **Sign in on this computer**. With one, it says **Signed in as
   `<account>` on this computer.**

## 7. Pass criteria

- Automatic never mentions the device flow, in any state.
- Ask reports the device flow and what to do about it.
- Both modes show this computer's own sign-in state and offer the sign-in.

## 8. Cleanup

1. Keep the mode the person wants.

   Result: The settings match the intent. No cloud was started.

## 9. Record of results

Put the results in the pull request.
