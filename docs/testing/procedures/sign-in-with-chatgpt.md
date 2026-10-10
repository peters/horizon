---
procedure: sign-in-with-chatgpt
feature: Sign in with ChatGPT for Codex (Cloud settings)
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: [a ChatGPT account for the sign-in, stored under the cloud root in owner-only files]
owner: peters
---

# Sign in with ChatGPT test procedure

## 1. Purpose

This procedure proves that a person signs Codex in with a ChatGPT account from
Cloud settings, that the saved sign-in shows the account and the plan state,
that the first plan-usage notice appears once, that a new cloud setting saves
with the ChatGPT mode, and that sign-out revokes the session and clears the
stored tokens.

## 2. Applicability

- Candidate: a Horizon build with Sign in with ChatGPT in the Coding agents
  card of Cloud settings.
- Platforms: Linux, macOS and Windows.
- This procedure does not test: worker handoff of the saved sign-in. That is a
  later feature. It also does not test the ChatGPT sign-in service itself.

## 3. Safety

> **CAUTION:** USE A TEST ACCOUNT WHEN AVAILABLE. The sign-in stores an account
> identifier and tokens under the cloud root, readable only by that user.

## 4. Equipment and preconditions

- A ChatGPT account that can sign in at `auth.openai.com`.
- A default web browser on the machine.
- Cloud settings that open. No provider key is needed for these tasks.

## 5. Setup

1. Open **Cloud settings**.

   Result: The **Coding agents** card lists **Codex** and **Claude**. Codex
   offers **API key**, **Subscription login** and **ChatGPT plan**. Claude
   offers **API key** and **Subscription login** only.

## 6. Tasks

### 6.1 C01 — Show the sign-in card

1. Select **Codex**. Select **ChatGPT plan**.

   Result: The card shows **Continue with ChatGPT** and the text that sign-in
   happens once, in the browser. The card status shows **Needs sign-in**.

2. Select **Subscription login**.

   Result: The card shows the subscription text again. **Claude** never shows
   a **ChatGPT plan** option.

### 6.2 C02 — Sign in

1. Select **ChatGPT plan**. Click **Continue with ChatGPT**.

   Result: The browser opens the ChatGPT sign-in page. The card shows the
   waiting text and **Cancel**.

2. Sign in with the ChatGPT account in the browser. Grant the requested
   scopes.

   Result: The card shows **Signed in as** with the account email and the
   **ChatGPT plan** mark when the `chatgpt.tokens.use.direct` scope is
   granted. The card status no longer shows **Needs sign-in**.

3. Examine the cloud root on the machine.

   Result: A `chatgpt` directory with an owner-only directory mode holds one
   file per sign-in and an `active` file. No settings file or deployment state
   contains an access token or a refresh token.

### 6.3 C03 — First plan-usage notice

1. Look at the connected card for an account granted plan usage.

   Result: The card shows **You're using your ChatGPT plan for eligible work**
   and a **Got it** button.

2. Click **Got it**. Reopen Cloud settings.

   Result: The notice does not appear again. The **Manage usage** link opens
   `https://chatgpt.com/settings/usage`.

### 6.4 C04 — Save the settings

1. With **ChatGPT plan** selected for Codex, click **Save settings**.

   Result: The settings save without an error. The settings file records the
   ChatGPT mode for Codex. Reopening Cloud settings keeps **ChatGPT plan**
   selected.

2. Sign out with C05 first, then select **ChatGPT plan** and click
   **Save settings** with no saved sign-in.

   Result: The save fails with the message to sign in with ChatGPT first.
   Nothing saves.

### 6.5 C05 — Sign out

1. On the connected card, click **Sign out**.

   Result: The card shows the signed-out message and the **Continue with
   ChatGPT** button again.

2. Examine the cloud root.

   Result: The sign-in file keeps the account and the issued client id, but
   its token fields are empty. The `active` file no longer names the account.

### 6.6 C06 — Cancel a sign-in

1. Click **Continue with ChatGPT**. Before the browser sign-in finishes, click
   **Cancel** on the card.

   Result: The waiting text goes away. The loopback callback server ends. A
   later code delivery to the callback does not store anything.

2. Repeat the sign-in once more.

   Result: A new browser page opens and a new sign-in completes normally.

## 7. Pass criteria

- Every task above shows its Result.
- No token value appears in any settings file, deployment state, log or UI
  text.
- The stored files keep owner-only permissions on every platform.

## 8. Cleanup

1. If the test account signed in, click **Sign out** on the card.

   Result: The card shows the **Continue with ChatGPT** button again.

2. Close Cloud settings.

   Result: The settings page closes.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
