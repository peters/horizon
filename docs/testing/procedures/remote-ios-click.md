---
procedure: remote-ios-click
feature: Trusted clicks and field entry on remote iOS browsers
platforms: [linux, macos, windows]
cost: paid device
destructive: no
secrets: [remote provider credentials managed by the host]
owner: peters
---

# Remote iOS click test procedure

## 1. Purpose and boundary

This procedure checks trusted clicks and field entry through the public browser MCP tools.
The host can run on any supported platform. The browser must run on the selected physical
device. Use the exact candidate binary and record its commit and SHA-256.

Page selectors, DOM state and script results are untrusted. The geometry check must reject
missing, hidden and disabled targets before native input. A successful geometry check is
not proof that the input was delivered. Read the visible result after each action.

This change keeps geometry synchronous when no field clearing is requested. It keeps the
asynchronous focus and input-handler checks for local clearing. It does not change native
click dispatch, element reference ownership, device coordinates or evaluation timeout policy.

## 2. Preconditions

1. Run the script regression check.

   ```bash
   node --test scripts/browser-smoke/target_rect.test.cjs
   ```

   Result: Geometry serializes as an immediate object. Local clearing still waits for
   event handlers. Disabled, hidden and missing elements remain errors.

2. Start an isolated Horizon host, or its documented standalone remote MCP host.

   Result: The shared desktop instance is unchanged. Credentials stay in the host's
   configured store or private files. Do not put them in commands or reports.

3. Use `scripts/browser-smoke/fixtures/remote-input.html` as the synthetic page.

   The counter starts at zero. Each click increments it once and shows whether the event
   was trusted. The page has a text input and disabled and hidden buttons.

   Result: The page is reachable by the device. A `data:text/html` URL can carry this
   fixture when the device accepts data URLs.

   If the device refuses data URLs, serve the file from an approved HTTPS test host.
   The public [Selenium web form](https://www.selenium.dev/selenium/web/web-form.html)
   is an alternative for field entry, a disabled input and a Submit button.
   With that page, replace the counter check with the visible `Form submitted` and
   `Received!` messages. Record this substitution; it does not measure the event counter.

## 3. Checks

### 3.1 IOS-CLICK-01: Physical iPhone

> **CAUTION:** USE ONLY ONE APPROVED DEVICE SESSION AT A TIME. Each session uses paid
> capacity. Release it before you open another session.

1. Open the synthetic page on an iPhone with Chrome through `browser_create`.

   Use a target from `browser_provider_devices`. Record the device and operating system from `remote_device`.

   Result: The provider confirms a physical device.

2. Click the button with `browser_act` and a fresh reference from `browser_snapshot`.

   Read the result with a new snapshot.

   Result: The counter is one and the page reports a trusted event.

3. Click the button with its selector after a fresh query.

   Read the result with a new snapshot.

   Result: The counter is two. No `invalid element coordinates` error occurs.

4. Fill the input with a synthetic value and a fresh reference.

   Read its visible result.

   Result: The input retains the value. The geometry check does not serialize a promise.

5. Click the disabled button.

   Result: The action fails with `element_disabled`. The counter stays at two.

6. Close the session.

   Examine allocations. Reconcile the exact reference if required.

   Result: The allocation is released before the next session starts.

7. Repeat steps 1 to 6 with Safari on a physical iPhone.

   Result: The same checks pass.

### 3.2 IOS-CLICK-02: Other engines

> **CAUTION:** THESE CHECKS USE PAID DEVICE CAPACITY. Close each session after its check.

1. Repeat IOS-CLICK-01 steps 1 to 6 with Android Chrome and a desktop browser.

   Result: Trusted clicks and field entry still work.

### 3.3 IOS-CLICK-03: Evaluation on the same engine

1. Call `browser_evaluate` with the constant `1+1` on the iOS session before you close it.

   Result: The result is `2`. The evaluation contract has its own procedure,
   [remote-ios-evaluation](remote-ios-evaluation.md). Do not repeat the promise and
   deadline checks here.

## 4. Cleanup and report

Close all owned sessions and the isolated host. Remove private credential files after their
last use. Record each check, actual device, OS, browser, commit and result in the PR or
`docs/testing/reports/`. Keep application identifiers, provider session IDs, private paths
and credentials out of public evidence. A tool acceptance response alone is not a pass.
