---
procedure: sign-in-with-chatgpt
feature: Sign in with ChatGPT for Codex (Cloud settings)
platforms: [linux, macos, windows]
cost: none
destructive: yes
secrets: [a test ChatGPT account for sign-in]
owner: peters
---

# Sign in with ChatGPT test procedure

## 1. Purpose

This procedure tests the local ChatGPT sign-in for Codex in Cloud settings.
It checks the saved account, the plan notice, settings save, cancellation and
sign-out. Codex offers **API key** and **ChatGPT plan** only. A saved Codex
subscription choice changes to **ChatGPT plan** when the form opens.

Settings saved by this build include `openai_auth`. Older builds that do not
recognize this field refuse the settings file, also after an API-key save.
Keep a private backup of the old settings before a downgrade test. Do not copy
the new settings file to a machine with an older build.

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
   offers **API key** and **ChatGPT plan**. Claude offers **API key** and
   **Subscription login**.

2. If a saved setting chose the old Codex subscription login, examine the
   Codex row after step 1.

   Result: **ChatGPT plan** is selected for Codex, and the card asks for the
   sign-in. Save requires the new sign-in when Codex is selected.

## 6. Tasks

### 6.1 C01 — Show the sign-in card

1. Select **Codex**. Select **ChatGPT plan**.

   Result: The card shows **Continue with ChatGPT** and the text that sign-in
   happens once, in the browser. The card status shows **Needs sign-in**.

2. Select **API key** for Codex.

   Result: The card shows the API key field. **Claude** keeps **Subscription
   login** and never shows a **ChatGPT plan** option.

### 6.2 C02 — Sign in

1. Select **ChatGPT plan**. Click **Continue with ChatGPT**.

   Result: The browser opens the ChatGPT sign-in page. The card shows the
   waiting text and **Cancel**.

> **CAUTION:** USE A TEST ACCOUNT. These steps give Horizon account access.
> Grant only the requested scopes.

2. Sign in with the ChatGPT account in the browser. Grant the requested
   scopes.

   Result: The card shows **Signed in as** with the account email and the
   **ChatGPT plan** mark when the `chatgpt.tokens.use.direct` scope is
   granted. The card status no longer shows **Needs sign-in**.

3. Examine the cloud root on the machine.

   Result: A `chatgpt` directory with owner-only access permissions holds one
   file per sign-in and an `active` file. No settings file or deployment state
   contains an access token or a refresh token.

### 6.3 C03 — First plan-usage notice

1. Look at the connected card for an account granted plan usage.

   Result: The card shows **Your ChatGPT plan is connected on this computer**
   and a **Got it** button.

2. Click **Got it**. Reopen Cloud settings.

   Result: The notice does not appear again. The **Manage usage** link opens
   `https://chatgpt.com/settings/usage`.

### 6.4 C04 — Save the settings

For this task, use a private test configuration with one compute provider set.
Settings save requires a provider. Do not create or deploy a cloud.

1. With **ChatGPT plan** selected for Codex, click **Save settings**.

   Result: The settings save without an error. The settings file records the
   ChatGPT mode for Codex. Reopening Cloud settings keeps **ChatGPT plan**
   selected.

2. With an account that has no plan access, click **Save settings**.

   Result: The card shows **Needs plan access**. The save fails. The saved
   settings and API key files do not change.

3. Sign out with C05 first, then select **ChatGPT plan** and click
   **Save settings** with no saved sign-in.

   Result: The save fails with the message to sign in and grant plan access.
   Nothing saves.

### 6.5 C05 — Sign out

> **CAUTION:** SIGN OUT ONLY THE TEST ACCOUNT. This step clears local tokens
> and asks the service to revoke this connection.

1. On the connected card, click **Sign out**.

   Result: The card shows the signed-out message and the **Continue with
   ChatGPT** button again.

2. Examine the cloud root.

   Result: The sign-in file keeps the account and the issued client id, but
   its token fields are empty. The `active` file no longer names the account.
   If remote revocation fails, the card reports that local sign-out is complete
   and that remote revocation is not confirmed.

3. Click **Continue with ChatGPT** after sign-out.

   Result: A new registration permits a different account. The old registration
   keeps its original account identity.

### 6.6 C06 — Cancel a sign-in

1. Click **Continue with ChatGPT**. Before the browser sign-in finishes, click
   **Cancel** on the card.

   Result: The waiting text goes away. The loopback callback server ends. A
   later code delivery to the callback does not store anything.

2. Repeat C02.

   Result: A new browser page opens and a new sign-in completes normally.

### 6.7 C07 — Account status unavailable

1. Open Cloud settings while another operation holds the account lock.

   Result: The card reports that the saved account status is unavailable.
   Save reports the status error. It does not claim that sign-out succeeded.

2. After the other operation ends, reopen Cloud settings.

   Result: The card reads the saved account again.


### 6.8 C08 — Another account becomes selected before sign-out

1. Load the card for test account A. Select test account B through another
   authorized test operation before you click **Sign out** for A.

   Result: The tokens for A are cleared. B remains selected. The card shows B
   and reports that another account is still selected.

2. Examine the record for B.

   Result: Its credentials do not change. The card does not report that all
   plan use has stopped.

### 6.9 C09 — Verify macOS credential ACLs

Run this lane on macOS. Linux tests do not qualify it.

1. Run `cargo test -p horizon-core cloud_runtime::chatgpt::store::macos`.
   Result: The directory, file and inherited ACL regressions pass.
2. Confirm that the tests add an allow entry for the system `nobody` account
   only on disposable objects with synthetic data.
   Result: The mode bits stay private, but readers refuse the extended ACL.
3. Confirm that an atomic replacement and a new credential directory have no
   extended ACL after an owned write.
   Result: New token bytes are written only after ACL protection.

The system `/bin/ls` and `/bin/chmod` are required. A permission command failure,
warning, oversized output or timeout must stop credential access.

## 7. Pass criteria

- Every task above shows its Result.
- No token value appears in any settings file, deployment state, log or UI
  text.
- On Unix, the stored directory mode is `0700` and the file mode is `0600`.
  Credential reads must reject access for other users and a different directory owner.
- The settings regression test must reject a stale or fabricated account status.
  The existing API-key settings and key file must remain unchanged.
- After a failed or interrupted sign-out, the UI must keep the last known account
  and show an error. It must not report successful sign-out.
- Local tokens must be cleared before remote revocation starts. A local failure
  must not send the revocation request. The session lock must be free during
  remote revocation.
- Discovery endpoints must use HTTPS on the trusted provider origin. Requests
  must not follow redirects to another origin.
- The token-response tests must reject empty access and refresh tokens.
  They must reject missing or unsupported token types. Bearer is case-insensitive.
- If a sign-in or usage-confirmation worker ends unexpectedly, the card must
  show an error and release the form. A failed sign-in must stop its callback.
- ID-token tests must reject a missing or empty key ID, even with a valid
  signature from a key that also has no key ID.
- Signed ID-token tests must reject a missing or malformed issuance time.
  They must accept a time 60 seconds in the future and reject 61 seconds.
  Expired and not-yet-valid tokens must still fail.
- The sign-in publication tests must recover an interruption before or after the
  credential commit. An activation failure must not publish new credentials.
- The snapshot test must prevent a concurrent writer between the record read and
  the active account read. Concurrent readers must fail closed while the writer
  holds the session lock.
- If the active account record is missing, status, sign-in and settings save
  must fail. Another saved account must not become selected. The remaining
  credential records and the active account selection must not change.
- A second first-time sign-in must not finish after another account becomes
  active. A pending sign-in must refuse changed tokens, sign-out or selection.
  The winning record and account selection must remain unchanged.
- The callback tests must enforce the deadline when connections are queued or a
  client sends an incomplete request. A partial HTTP callback must be refused.
- The refresh tests must prevent an early provider request. They must replace both
  tokens on success and preserve the record on a refused or malformed response.
- The Unix lock test must permit a new operation while a copied descriptor stays
  open after the preceding operation ends.
- On Windows, use the system Windows PowerShell installation. The saved
  directory and each credential file must have a protected discretionary
  access control list (DACL). It must grant full control only to the current
  user security identifier (SID). The Windows regression test
  must reject a file and a directory after another SID gains read access.
- Record real-account sign-in separately from tests with synthetic credentials.
  A synthetic fixture proves local card states, not provider authentication.

## 8. Cleanup

> **CAUTION:** CLEAR ONLY THE TEST CONNECTION. Sign-out clears local tokens
> and asks the service to revoke this connection.

1. If the test account signed in, click **Sign out** on the card.

   Result: The card shows the **Continue with ChatGPT** button again.

2. Close Cloud settings.

   Result: The settings page closes.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
