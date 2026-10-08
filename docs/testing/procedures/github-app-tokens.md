---
procedure: github-app-tokens
feature: GitHub App user tokens (device sign-in, renewal, refusal and expiry)
platforms: [linux, macos, windows]
cost: none
destructive: yes
secrets: [a GitHub account that can create a GitHub App]
owner: peters
---

# GitHub App user tokens test procedure

## 1. Purpose

This procedure proves that the GitHub client of Horizon signs in with the device
flow, renews a chain without a client secret, and reports a refusal and an expired
sign-in. The checks use a real GitHub App that the tester creates and deletes.

## 2. Applicability

- Candidate: a Horizon build that contains `horizon_cloud::github`.
- Platforms: Linux, macOS and Windows. The example runs in a terminal.
- This procedure does not test: the web sign-in, the manifest flow, the settings
  and the cloud card. The Connect GitHub procedure tests them when the user
  interface is available.

## 3. Safety

> **CAUTION:** DELETE THE TEST APP IN THE CLEANUP. A GitHub App that you do not use
> stays on your account and can receive new authorizations.

> **CAUTION:** DO NOT COPY A TOKEN INTO THE REPORT. The example prints only the
> lifetimes and HTTP status codes. Keep it that way.

## 4. Equipment and preconditions

- A GitHub account. Use a test account if one is available.
- A Rust toolchain and a checkout of the candidate.
- No other preconditions.

## 5. Setup

1. On GitHub, open **Settings › Developer settings › GitHub Apps › New GitHub App**.

   Result: GitHub shows the registration form.

2. Type a name such as `horizon-token-test-<date>`.

   Result: The name field shows the name.

3. Type any homepage URL.

   Result: The homepage field shows the URL.

4. Type `http://127.0.0.1/callback` as the callback URL.

   Result: The callback field shows the URL.

5. Make sure that **Expire user access tokens** is on.

   Result: The box is selected.

6. Select **Enable Device Flow**.

   Result: The box is selected.

7. Clear **Webhook › Active**.

   Result: The box is clear.

8. Click **Create GitHub App**.

   Result: GitHub shows the app settings with a client ID that starts with `Iv`.

9. Record the client ID in the private evidence.

   Result: You have the client ID.

## 6. Tasks

### 6.1 T01 — Sign in with the device flow and renew without a secret

1. In the checkout, run the example with the client ID.

   On Linux and macOS:

   ```sh
   HORIZON_GITHUB_CLIENT_ID=<client id> cargo run -p horizon-cloud --example github_token_smoke
   ```

   On Windows, in PowerShell:

   ```powershell
   $env:HORIZON_GITHUB_CLIENT_ID = "<client id>"; cargo run -p horizon-cloud --example github_token_smoke
   ```

   Result: The example shows `Open https://github.com/login/device and enter` and
   a code.

2. Open the URL in a browser.

   Result: GitHub asks for the code.

3. Type the code and click **Continue**.

   Result: GitHub shows the app and **Authorize**.

   > **CAUTION:** AUTHORIZE ONLY THE TEST APP THAT YOU MADE IN THE SETUP. Authorize
   > gives the app access to your account until you delete it in the cleanup.

4. Click **Authorize**.

   Result: The example shows `signed in: access token for 7 h, refresh token for 181 days`
   or 182 days.

5. Read the next lines.

   Result: The example shows these lines:

   - `first access token: HTTP 200`
   - `renewed without a client secret: access token for 7 h, ...`
   - `new access token: HTTP 200`
   - `old access token: HTTP 401`
   - `old refresh token: GitHub no longer accepts this sign-in. Connect GitHub again.`

### 6.2 T02 — Decline a sign-in

1. Run the example again, as in T01 step 1.

   Result: The example shows a new code.

2. Open the URL and type the code.

   Result: GitHub shows the app and **Cancel**.

3. Click **Cancel**.

   Result: The example shows `sign-in ended: The GitHub sign-in was declined.`

### 6.3 T03 — Let a sign-in expire

1. Run the example again. Do not enter the code.

   Result: The example shows a code and waits.

2. Wait 15 minutes.

   Result: The example shows
   `sign-in ended: The GitHub sign-in expired before it was approved. Start it again.`

### 6.4 T04 — Refuse a device sign-in when the app does not permit it

1. In the app settings, clear **Enable Device Flow**.

   Result: The box is clear.

2. Click **Save changes**.

   Result: GitHub saves the settings.

3. Run the example again.

   Result: The example shows `start:` and
   `Device sign-in is off for this GitHub App. Turn on Enable Device Flow in the app's settings.`

## 7. Pass criteria

- T01 renews the chain without a client secret.
- After the renewal, the old access token gets HTTP 401 and the old refresh token
  is refused.
- T02, T03 and T04 each show their own error text.
- No token is in the terminal output.

## 8. Cleanup

1. In the app settings, open **Advanced**.

   Result: GitHub shows **Delete GitHub App**.

2. Click **Delete GitHub App**.

   Result: GitHub asks for the name of the app.

   > **CAUTION:** MAKE SURE THAT THE NAME IS THE NAME OF THE TEST APP. Deleting an app
   > cannot be undone and ends every token of that app at once.

3. Type the name and confirm.

   Result: GitHub deletes the app. All tokens of the app stop working.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep the client ID in the private evidence only.
