---
procedure: browser-site-checks
feature: Browser key presses for site checks
platforms: [linux, windows, macos]
cost: none
destructive: no
secrets: none
owner: peters
---

# Browser site checks test procedure

## 1. Purpose

This procedure tests the `browser_act` `press` action. Site smoke tests use it
to examine the keyboard focus order and the focus ring of a page.

## 2. Applicability

- Use the exact candidate for the pull request.
- Test local Chromium and local Firefox on each available platform.
- Safari and remote targets use classic WebDriver actions. A real mobile device
  can ignore focus keys such as `Tab`.
- This procedure does not test text entry. Text entry uses `fill`.

## 3. Equipment and preconditions

- A task-owned isolated desktop with a live Device panel.
- The candidate, Chromium and Firefox.
- The fixture at `docs/testing/fixtures/browser-keyboard.html`.
- A local HTTP server that serves the fixture on a loopback address.
- A public browser MCP server for the candidate and its private coordination root.

## 4. Setup

1. Start the isolated desktop with `scripts/device-smoke/serve.py --native-view`.

   Result: The fixture reports its display and VNC endpoint.

2. Open a Device panel for that endpoint with `device_panel`.

   Result: The panel shows changing frames from the isolated desktop.

3. Open the fixture in a Chromium panel with `browser_create`.

   Result: The page shows **Section one**, **Section two** and **Menu**.

## 5. Tasks

### 5.1 K01 — Move focus forward with Tab

1. Send `browser_act` with `action: press` and `key: Tab`.

   Result: The action completes. **Skip to content** is visible and has a red focus ring.

2. Read `document.activeElement.textContent` and
   `document.activeElement.matches(':focus-visible')` with `browser_evaluate`.

   Result: The text is `Skip to content` and the match is `true`.

3. Send `browser_act` with `action: press`, `key: Tab` and `count: 3`.

   Result: The focus is on **Menu**. The status line shows `focus: menu`.

### 5.2 K02 — Move focus back and use a chord

1. Send `browser_act` with `action: press` and `key: Shift+Tab`.

   Result: The focus is on **Section two**.

2. Fill the **Name** field with `synthetic` through `browser_act` `fill`.

   Result: The field contains `synthetic`.

3. Send `browser_act` with `action: press` and `key: Control+a`. On macOS, use `Meta+a`.

   Result: All text in the field is selected. `selectionStart` is 0 and
   `selectionEnd` is 9 on the field.

4. Send `browser_act` with `action: press` and `key: Escape`.

   Result: The action completes and the page does not change.

### 5.3 K03 — Refuse a bad request

1. Send `browser_act` with `action: press` and `key: Hyper+Tab`.

   Result: The server refuses the request with `invalid_input`.

2. Send `browser_act` with `action: press`, `key: Tab` and a `selector`.

   Result: The server refuses the request with `invalid_input`.

3. Send `browser_act` with `action: press`, `key: Tab` and `count: 101`.

   Result: The server refuses the request with `invalid_input`.

### 5.4 K04 — Examine the audit log

1. Send `browser_act` with `action: press` and `key: q`.

   Result: The action completes.

2. Read the panel audit records with `browser_audit`.

   Result: The `press` records show `Tab` for named keys and `printable` for `q`.
   No record shows the pressed key as `q`.

### 5.5 K05 — Repeat in Firefox

1. Close the Chromium panel with `browser_close`.

   Result: The panel leaves `browser_list`.

2. Open the fixture in a Firefox panel with `browser_create` and `backend: firefox`.

   Result: The page shows **Section one**, **Section two** and **Menu**.

3. Do K01 and K02 in the Firefox panel.

   Result: The results are the same as in Chromium.

## 6. Pass criteria

- K01 to K05 give the specified results in Chromium and Firefox.
- No audit record contains a printable key.

## 7. Cleanup

1. Close each browser panel that this procedure opened with `browser_close`.

   Result: The panels leave `browser_list`.

2. Stop the isolated desktop.

   Result: The fixture process stops.

## 8. Record of results

Put the results in the pull request. Keep private evidence out of the repository.
