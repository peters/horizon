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
It also tests the preload fallback and one synthetic sign-in check.
It also tests the disclosure status on the local picker and the remote identity hover.

## 2. Applicability

- Use the candidate source checkout.
- On Linux, Firefox and geckodriver must be on PATH.
- Tasks 6.1 and 6.6 need geckodriver 0.37 or newer.
- System access stays off unless `firefox_system_access` is true.
- Task 6.6 sets that field to true in the fixture config.
- This procedure does not test Chromium or Safari.
- This procedure sends one synthetic Google identifier.
- This procedure does not send a password.
- This procedure does not click Continue on X.

## 3. Safety

> **CAUTION:** CLICK NEXT ONE TIME WITH THE SYNTHETIC EMAIL ONLY, AND DO NOT TYPE A PASSWORD.
>
> **CAUTION:** USE ONLY THE ISOLATED DESKTOP. A click on the developer desktop can change a real session.
>
> **CAUTION:** TASK 6.6 SETS `firefox_system_access` TO TRUE IN THE FIXTURE ONLY. A LOCAL CLIENT THAT REACHES THAT DRIVER PORT CAN USE FIREFOX UI PRIVILEGES.

## 4. Equipment and preconditions

- Rust and the workspace build tools.
- Local Firefox and geckodriver on PATH.
- For tasks 6.1 and 6.6, geckodriver 0.37 or newer.
- Permission to bind a loopback port.
- Permission to start a headless Firefox process.
- For task 6.6, a frozen Horizon candidate, `horizon-device` from the same commit, `xdotool`, `jq`, and the local device fixture.

## 5. Setup

1. Open a shell in the candidate checkout.

   Result: The shell is in the checkout.

2. If Firefox is a Snap, create the directory `~/tmp/horizon-firefox-disclosure`.

   Result: The directory exists. The path is under the home directory.

3. If Firefox is a Snap, set `TMPDIR` to `~/tmp/horizon-firefox-disclosure`.

   Result: The name does not start with a dot.

## 6. Tasks

### 6.1 NATIVE-GETTER — Native getter returns false

1. Run `cargo test -p horizon-browser --test firefox_native_flag_live -- --ignored --nocapture`.

   Result: The test passes. The page title is `false native`.
   The live test sets `firefox_system_access` to true.
   That title requires geckodriver 0.37 or newer.
   The panel reports `automation_disclosure` as `common_signals_minimized`.
   `BackendReady` carries that same status.

### 6.2 LAUNCH-POLICY — Both disclosure policies

1. Run `cargo test -p horizon-browser --lib firefox_screenshot_session_keeps_scrollbars_visible`.

   Result: A minimize session does not ask for `--allow-system-access`.
   The same session asks for that argument when `firefox_system_access` is true.
   A `BrowserDefault` session does not ask for that argument.
   Firefox options do not include `-remote-allow-system-access`.
   This test does not start geckodriver.

2. Run `cargo test -p horizon-browser --lib older_geckodriver_can_start_without_system_access`.

   Result: The test passes.
   The test matches `unexpected argument '--allow-system-access'`.
   The test matches `which wasn't expected`.
   The test does not match `address already in use`.
   The argument list includes `--allow-system-access` only when the permit and the opt-in are true.
   An unfinished stderr tail drops `--allow-system-access`.
   A finished unrelated error keeps that argument.
   This test does not start geckodriver.

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

### 6.6 SIGN-IN — Google checks the browser after Next

`device_panel` is the view only. Send input with `horizon-device` on the fixture target.

1. Start the local device fixture with `--native-view` and `--firefox-system-access`.

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <frozen-candidate> \
     --native-view --firefox-system-access --state <new-directory>
   ```

   Result: The fixture prints a loopback VNC address. `viewer_url` is null.
   The fixture config sets `browser.backend` to `firefox`.
   The fixture config sets `browser.firefox_system_access` to true.
   The workspace opens the Google page and the X page.

2. Record the candidate process ID.

   ```sh
   pgrep -f '^<frozen-candidate> --config'
   ```

   Result: The command shows one process ID.
   `/proc/<pid>/exe` is the frozen candidate.

3. Open that address with the public `device_panel` tool.

   Result: The Device panel shows the isolated desktop.
   The panel does not accept input.

4. Inspect the Device panel twice, two seconds apart.

   Result: The displayed frame advances.

5. Define the input helpers. `<state>/target.json` names the fixture display.

   ```sh
   device() { <bin>/horizon-device --target <state>/target.json "$@"; }
   geometry() {
     device screenshot --options '{"region":{"x":0,"y":0,"width":2,"height":2}}' \
       | jq -c .result.geometry
   }
   send_type() {
     jq -Rsc --argjson g "$(geometry)" '{geometry: $g, action: {kind: "type", text: .}}' \
       | device act -
   }
   send_key() {
     jq -nc --argjson g "$(geometry)" --arg k "$1" \
       '{geometry: $g, action: {kind: "key", key: $k, modifiers: []}}' | device act -
   }
   send_click() {
     jq -nc --argjson g "$(geometry)" --argjson x "$1" --argjson y "$2" \
       '{geometry: $g, action: {kind: "click", at: {x: $x, y: $y}, button: "left"}}' \
       | device act -
   }
   ```

   Result: The helpers use the fixture display. They do not use the developer display.

6. Focus the candidate window on the fixture display.

   ```sh
   DISPLAY=<display> xdotool search --pid <pid> --name '^Horizon$' windowactivate --sync
   ```

   Result: The window for that process ID is focused.

7. Capture the isolated desktop.

   ```sh
   device screenshot <evidence>/desktop.png
   ```

   Result: The image shows the Google email field and the X username field.

8. Click the Google email field at its surface point.

   ```sh
   send_click <email-x> <email-y>
   ```

   Result: The email field has focus.

9. Type the synthetic email.

   ```sh
   printf '%s' 'not-a-real-person@example.com' | send_type
   ```

   Result: The email field shows that text.

10. Press Enter one time. This is the one Next action.

    ```sh
    send_key enter
    ```

    Result: The page shows "Couldn't find this account".
    The page does not show "This browser or app may not be secure."

11. Do not press Enter again.

    Result: The password field stays empty.

12. Capture the X login page.

    ```sh
    device screenshot <evidence>/x-login.png
    ```

    Result: The image shows the username field.

13. Click the X username field at its surface point.

    ```sh
    send_click <username-x> <username-y>
    ```

    Result: The username field has focus.

14. Type the synthetic username.

    ```sh
    printf '%s' 'not-a-real-person' | send_type
    ```

    Result: The username field shows that text.

15. Leave the X login form as it is.

    Result: The login form is still open. Continue stays unused.

### 6.7 STATUS — Disclosure text at two panel widths

1. Run `cargo test -p horizon-ui --bin horizon disclosure_status_stays_visible_on_a_narrow_panel`.

   Result: The local picker tooltip shows `common_signals_minimized`.
   The remote identity tooltip shows `unsupported_by_backend`.
   Both strings stay inside the panel at width 720 and at width 280.

## 7. Pass criteria

- Task 6.1 reports the title `false native`.
- Tasks 6.2 through 6.5 pass.
- Task 6.6 starts with `browser.firefox_system_access` set to true.
- Task 6.6 shows "Couldn't find this account".
- Task 6.6 does not show "This browser or app may not be secure."
- Task 6.7 shows both disclosure strings at width 720 and at width 280.

## 8. Cleanup

1. Close the Firefox panel inside the isolated Horizon window.
2. Close the Device panel with the `device_panel` operation `close`.
3. Stop the fixture processes that this run started.

   Result: The candidate process stops. The developer desktop does not change.

## 9. Record of results

Put the results in the pull request. Do not commit private page evidence.
