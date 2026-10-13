---
procedure: browser-passkey-bridge
feature: Browser WebAuthn bridge to an external authenticator command
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Browser passkey bridge test procedure

## 1. Purpose

This procedure tests the WebAuthn bridge of browser panels. It proves that a
passkey request goes to the configured authenticator command, that the host
applies the origin rules, and that the panel shows a waiting request.

## 2. Applicability

- Candidate: the exact Horizon executable of the pull request.
- Platforms: Linux with local Chromium and local Firefox.
- The fixture of the isolated desktop runs Horizon in `bwrap`. A Snap browser
  does not start in it. Use a browser from a Debian package there, for
  example `/usr/lib/firefox/firefox`.
- This procedure does not test Safari, remote sessions or a real authenticator.
- The reference for the rules is
  [the WebAuthn bridge reference](../../architecture/browser-webauthn-bridge.md).

## 3. Safety

> **CAUTION:** USE ONLY THE FAKE AUTHENTICATOR AND SYNTHETIC ACCOUNTS. The
> test keys are not secret, so a real account with these keys is not safe.

## 4. Equipment and preconditions

- Chromium, Firefox and `geckodriver`.
- Docker, for the Keycloak task.
- Network access to `https://webauthn.io`.
- The fake authenticator. It is the live test executable of
  `crates/horizon-browser/tests/webauthn_live.rs`.
- The fixture in `scripts/device-smoke/serve.py` and a live Device panel.

## 5. Setup

1. Build the fake authenticator:

   ```sh
   cargo test -p horizon-browser --test webauthn_live --no-run
   ```

   Result: Cargo shows the path of the `webauthn_live` executable.

2. Copy the candidate, the fake authenticator and `geckodriver` to a private
   directory outside your home directory.

   Result: The directory has three frozen files. Record their SHA-256.

3. Start the fixture with `--native-view` and a new state directory.

   Result: The fixture reports its display and `vnc_address`.

4. Open a Device panel for the `vnc_address` with `device_panel`.

   Result: The Device panel shows frames that advance.

5. Make a directory `authenticator` in the state directory of the fixture.

   Result: The fake authenticator can keep its keys there.

6. Add this `browser` section to `data/horizon.yaml` of the fixture. Use the
   paths of step 2 and step 5.

   ```json
   {"backend": "firefox", "firefox_command": "/usr/lib/firefox/firefox",
    "geckodriver_command": "<dir>/geckodriver",
    "webauthn_command": ["<dir>/fake-authenticator", "--fake-authenticator", "<state>/authenticator"]}
   ```

   Result: The fixture log shows `config file changed`.

## 6. Tasks

### 6.1 P01 — Automated live test

1. Run the live test for both engines:

   ```sh
   cargo test -p horizon-browser --test webauthn_live -- --ignored
   ```

   Result: The output shows `chromium passed` and `firefox passed`.

2. If a browser is a Snap package, set `HORIZON_LIVE_PROFILE_ROOT` to a
   directory in your home directory. Then do step 1 again.

   Result: The browser starts with a profile that the Snap can read.

### 6.2 P02 — Bridge off by default

1. Open a browser panel in a fixture without `webauthn_command`.

2. Run `String(CredentialsContainer.prototype.get)` in the page.

   Result: The text contains `[native code]`. The bridge is off.

### 6.3 P03 — webauthn.io

1. Write `slow` in the file `authenticator/mode`.

   Result: The fake authenticator answers after three seconds.

2. Press **Ctrl+Shift+K**, type `browser` and press Enter.

   Result: A Firefox browser panel opens.

3. Open `https://webauthn.io` in the panel.

4. Type a synthetic username and click **Register**.

   Result: The panel chip shows **passkey request waiting**.

   Result: Then the page shows `Success! Now try to authenticate...`.

5. Click **Authenticate**.

   Result: The chip shows **passkey request waiting**. Then the page opens
   `https://webauthn.io/profile`.

6. Examine `authenticator/calls.log`.

   Result: The log has one `create` line and one `get` line for
   `https://webauthn.io`.

### 6.4 P04 — Local Keycloak

1. Start Keycloak 26 with `start-dev` on `127.0.0.1:18080`.

2. Make a realm, a synthetic user with a password and the required action
   `webauthn-register-passwordless`.

3. Make a browser flow with `auth-username-form` and
   `webauthn-authenticator-passwordless`, both `REQUIRED`. Do not bind it yet.

4. Open `http://localhost:18080/realms/<realm>/account` in the browser panel.

5. Sign in with the password and click **Register**.

   Result: Keycloak shows the account console. The user has a
   `webauthn-passwordless` credential.

6. Bind the new flow as the browser flow of the realm. Click **Sign out**.

7. Type the username and click **Sign in with Passkey**.

   Result: Keycloak shows the account console again.

8. Write `deny` in `authenticator/mode`. Sign out and do step 7 again.

   Result: Keycloak shows `Failed to authenticate by the Passkey.`

### 6.5 P05 — Origin rules and abort

1. Examine the result of P01.

   Result: The live test proves these rules in both engines:

   - A cross-origin iframe gets `NotAllowedError`.
   - `http://127.0.0.1` gets `SecurityError`.
   - A foreign `rpId` gets `SecurityError`.
   - Conditional mediation stays pending until an abort.
   - An abort kills the command process and clears the chip.
   - No refused request starts the command.

## 7. Pass criteria

- P01 passes for Chromium and Firefox.
- P02 shows the native function.
- P03 and P04 sign in with a passkey that the fake authenticator made.
- The chip shows **passkey request waiting** while the command runs and
  disappears after it.

## 8. Cleanup

1. Close the Device panel with `device_panel`.
2. Stop the fixture with its exact process ID.
3. Remove the Keycloak container.

   Result: No process or container of this run continues.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
