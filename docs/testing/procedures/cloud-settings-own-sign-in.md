---
procedure: cloud-settings-own-sign-in
feature: Cloud settings, this computer's GitHub sign-in
platforms: [linux, macos]
cost: none
destructive: no
secrets: [GitHub App secret in Cloud settings]
owner: peters
---

# Cloud settings own sign-in test procedure

## 1. Purpose

This procedure makes sure that the GitHub card of **Cloud settings** says whether
this computer is signed in, and that the card offers the sign-in itself when it is
not. It also makes sure that a sign-in started there names what GitHub asks for.

## 2. Applicability

- Candidate: a build whose GitHub card shows **Signed in as …** or **No sign-in on
  this computer yet**.
- Platforms: Linux and macOS.
- This procedure does not test: creating the app, the mode, the repositories and
  the cloud's own sign-in.

## 3. Safety

No step rents compute, deletes data or changes access. A started sign-in stores a
sign-in only when the person approves it, and **Cancel** ends it.

## 4. Equipment and preconditions

- One Horizon window.
- A connected GitHub app in Cloud settings.
- Optionally, a browser that is signed in to GitHub, to finish a sign-in.

## 5. Setup

1. Note the file `~/.horizon/cloud/credentials/github-host-<app id>.json`.

   Result: The file exists when this computer is signed in, and it does not exist
   when it is not.

## 6. Tasks

### 6.1 OWN-SIGN-IN-1 — The card tells the truth about this computer

1. Open **Cloud settings** and read the GitHub card.

   Result: With the file from step 1 of section 5 present, the card says
   **Signed in as `<account>` on this computer.** Without the file, it says
   **No sign-in on this computer yet. A new cloud cannot get GitHub access until
   it has one.**, in yellow.

2. Compare the two states against the file.

   Result: The card never says **Signed in as** while the file is absent, and it
   never says **No sign-in** while the file is present.

### 6.2 OWN-SIGN-IN-2 — The card offers the sign-in

1. With no sign-in on this computer, press **Sign in on this computer**.

   Result: The card shows what GitHub asks for: **Approve the sign-in in the
   browser Horizon opened.** with **Open the page**, or **Enter `<code>` at
   `<address>`** with **Open the page**.

2. Press **Open the page**.

   Result: The page opens in the browser.

3. Press **Cancel** in the dialog.

   Result: The dialog closes and the sign-in ends. The file of section 5 is still
   absent unless the browser step was completed.

### 6.3 OWN-SIGN-IN-3 — A finished sign-in shows its account

1. Open **Cloud settings** again and press **Sign in on this computer**.
2. Finish the approval in the browser.

   Result: The card says **Signed in as `<account>` on this computer.**, and the
   file of section 5 now exists.

## 7. Pass criteria

- The card's line matches whether this computer holds a sign-in.
- **Sign in on this computer** appears exactly when there is no sign-in, and
  starts a sign-in that names what GitHub asks for.
- **Cancel** ends a started sign-in and stores nothing.
- A finished sign-in shows its account without reopening the dialog.

## 8. Cleanup

1. Keep the sign-in if the person wants it, or press **Disconnect** to end it.

   Result: The state of section 5 matches what the card says.

## 9. Record of results

Put the results in the pull request.
