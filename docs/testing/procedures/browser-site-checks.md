---
procedure: browser-site-checks
feature: Browser key presses and page zoom for site checks
platforms: [linux, macos]
cost: none
destructive: no
secrets: none
owner: peters
---

# Browser site checks test procedure

## 1. Purpose

This procedure tests the `browser_act` `press` action and the `browser_zoom`
tool. Site smoke tests use them to examine the keyboard focus order, the focus
ring, and the reflow of a page at a high zoom.

## 2. Applicability

- Use the exact candidate for the pull request.
- Test local Chromium and local Firefox on each available platform.
- Safari and remote targets use classic WebDriver actions. A real mobile device
  can ignore focus keys such as `Tab`.
- Safari and remote targets refuse `browser_zoom` with `unsupported_backend`.
- This procedure does not test text entry. Text entry uses `fill`.

## 3. Equipment and preconditions

- The candidate `horizon-browser` binary, Chromium, Firefox and geckodriver.
- The fixture at `docs/testing/fixtures/browser-keyboard.html`.
- A stdio MCP client, for example a short script that sends JSON-RPC lines.
- An empty folder for the home folder of the MCP server. It keeps the browser
  profile and the coordination files away from your own.

## 4. Setup

1. In the repository root, start the fixture server:
   `python3 -m http.server 18731 --bind 127.0.0.1 --directory docs/testing/fixtures`.

   Result: `http://127.0.0.1:18731/browser-keyboard.html` returns the fixture.

2. From the MCP client, start the candidate MCP server:
   `env -u HORIZON -u HORIZON_BROWSER_ACTOR HOME=<empty folder> horizon-browser mcp --standalone --backend chromium`.
   Remove all `CLAUDE_CODE_*` variables from its environment too.

   Result: After `initialize`, `browser_list` shows one `standalone-` panel.
   Its capabilities contain `press`.

3. Call `browser_navigate` with that panel ID and the fixture URL from step 1.

   Result: The page commits. `browser_query` for `#menu` finds the **Menu** button.

## 5. Tasks

### 5.1 K01 — Move focus forward with Tab

1. Send `browser_act` with `action: press` and `key: Tab`.

   Result: The action completes.

2. Read `document.activeElement.textContent` and
   `document.activeElement.matches(':focus-visible')` with `browser_evaluate`.

   Result: The text is `Skip to content` and the match is `true`. The fixture
   draws a red focus ring for `:focus-visible`.

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

### 5.5 Z01 — Zoom to 200 percent

Do this task in the Chromium panel. K05 repeats it in Firefox.

1. Call `browser_resize` with `width: 1280` and `height: 800`.

   Result: `applied` is 1280 x 800 and `zoom_percent` is 100. The fixture shows
   `width: 1280 CSS px · columns: 3`.

2. Call `browser_zoom` with `percent: 200`.

   Result: `applied_percent` is 200 and `css_viewport` is 640 x 400. The fixture
   shows `width: 640 CSS px · columns: 1`.

3. Read `[document.documentElement.scrollWidth, document.documentElement.clientWidth]` with `browser_evaluate`.

   Result: The two values are equal. No content is clipped horizontally.

### 5.6 Z02 — Combine zoom with a pin and reset

1. Call `browser_zoom` with `percent: 400`.

   Result: `css_viewport` is 320 x 200.

2. Call `browser_resize` with `width: 1000` and `height: 800`.

   Result: `applied` is 250 x 200 and `zoom_percent` is 400.

3. Call `browser_zoom` with `reset: true`.

   Result: `requested_percent` is null, `applied_percent` is 100 and
   `css_viewport` is 1000 x 800.

4. Call `browser_resize` with `reset: true`.

   Result: The page follows the panel size again.

5. Call `browser_zoom` with `percent: 501`.

   Result: The server refuses the request with `invalid_input`.

### 5.7 K05 — Repeat in Firefox

1. Close the input of the Chromium MCP server.

   Result: The server stops its browser and deletes the profile.

2. Start the MCP server again with `--backend firefox` and navigate to the fixture URL.

   Result: `browser_list` shows one Firefox panel with the `press` capability.

3. Do K01, K02, Z01 and Z02 in the Firefox panel.

   Result: The results are the same as in Chromium.

## 6. Pass criteria

- K01 to K05, Z01 and Z02 give the specified results in Chromium and Firefox.
- No audit record contains a printable key.

## 7. Cleanup

1. Close the input of the Firefox MCP server.

   Result: The server stops its browser and deletes the profile.

2. Stop the fixture server from step 1 of the setup.

   Result: The port is free.

3. Delete the empty home folder.

   Result: No browser profile from this procedure remains.

## 8. Record of results

Put the results in the pull request. Keep private evidence out of the repository.
