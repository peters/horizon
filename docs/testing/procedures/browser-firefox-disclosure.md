---
procedure: browser-firefox-disclosure
feature: Firefox native webdriver getter
platforms: [linux]
cost: none
destructive: no
secrets: synthetic values only
owner: peters
---

# Firefox disclosure test procedure

## 1. Purpose

This procedure tests the native Firefox `navigator.webdriver` getter.
It also tests the preload fallback and a sign-in field on a browser panel.

## 2. Applicability

- Use the candidate source checkout.
- On Linux, Firefox and geckodriver must be on PATH.
- This procedure does not test Chromium or Safari.
- This procedure does not send a login form.

## 3. Safety

> **CAUTION:** USE ONLY SYNTHETIC ACCOUNT TEXT. A real password or a login click can lock the account.
>
> **CAUTION:** USE ONLY THE ISOLATED DESKTOP. A click on the developer desktop can change a real session.

## 4. Equipment and preconditions

- Rust and the workspace build tools.
- Local Firefox and geckodriver on PATH.
- Permission to bind a loopback port.
- Permission to start a headless Firefox process.
- For task 6.6, a frozen Horizon candidate and the local device fixture.

## 5. Setup

1. Open a shell in the candidate checkout.

   Result: The shell is in the checkout.

2. If Firefox is a Snap, set `TMPDIR` to `~/tmp/horizon-firefox-disclosure`.

   Result: The path is under the home directory. The name does not start with a dot.

## 6. Tasks

### 6.1 NATIVE-GETTER — Native getter returns false

1. Run `cargo test -p horizon-browser --test firefox_native_flag_live -- --ignored --nocapture`.

   Result: The test passes. The page title is `false native`.

### 6.2 LAUNCH-POLICY — Both disclosure policies

1. Run `cargo test -p horizon-browser --lib firefox_screenshot_session_keeps_scrollbars_visible`.

   Result: A minimize session starts geckodriver with `--allow-system-access`.
   A `BrowserDefault` session does not start geckodriver with that argument.
   Firefox options do not include `-remote-allow-system-access`.

### 6.3 SHARED-CLEAR — One clear for a shared process

1. Run `cargo test -p horizon-browser --lib shared_process_clears_the_native_automation_flag_once`.

   Result: The session transport gets the chrome switch, the flag script, and the content switch one time.
   A second call does not send those commands.
   The page host refuses a command to `moz/context`.

### 6.4 FALLBACK — Preload when the chrome context is absent

1. Run `cargo test -p horizon-browser --lib shared_process_uses_the_preload_fallback_when_chrome_context_is_unavailable`.

   Result: The chrome switch fails.
   The clear returns the session to the content context.
   The native flag stays set, so the preload shim can run.

### 6.5 UNSAFE — Stop a session stuck in the chrome context

1. Run `cargo test -p horizon-browser --lib shared_process_rejects_a_session_stuck_in_chrome_context`.

   Result: The clear returns an error.
   The clear does not mark the process as cleared.

### 6.6 SIGN-IN — A panel field accepts synthetic text

1. Start the local device fixture with the frozen candidate and `--native-view`.

   Result: The fixture prints a loopback VNC address. `viewer_url` is null.

2. Open that address with the public `device_panel` tool.

   Result: The Device panel shows the isolated desktop.

3. Inspect the Device panel twice, two seconds apart.

   Result: The displayed frame advances.

4. In the isolated Horizon window, open a Firefox browser panel.

   Result: The panel is ready.

5. Go to `https://accounts.google.com/ServiceLogin?hl=en`.

   Result: The Google sign-in form is open.

6. Type `not-a-real-person@example.com` in the email field.

   Result: The email field shows that text.

7. Do not click Next.

   Result: The page does not show the text "This browser or app may not be secure."

8. Go to `https://x.com/i/flow/login`.

   Result: The X login form is open.

9. Type `not-a-real-person` in the username field.

   Result: The username field shows that text.

10. Do not click Continue.

    Result: The login form is still open.

## 7. Pass criteria

- Task 6.1 reports the title `false native`.
- Tasks 6.2 through 6.5 pass.
- Task 6.6 shows the synthetic text.
- Task 6.6 does not show the insecure-browser message.

## 8. Cleanup

1. Close the Firefox panel inside the isolated Horizon window.
2. Close the Device panel with the `device_panel` operation `close`.
3. Stop the fixture processes that this run started.

   Result: The candidate process stops. The developer desktop does not change.

## 9. Record of results

Put the results in the pull request. Do not commit private page evidence.
