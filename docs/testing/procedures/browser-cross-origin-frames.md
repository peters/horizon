---
procedure: browser-cross-origin-frames
feature: Browser semantic input in child frames
platforms: [linux, macos]
cost: none
destructive: no
secrets: synthetic values only
owner: peters
---

# Browser child-frame input test procedure

## 1. Purpose

This procedure tests form input in cross-origin frames through public MCP tools.
It tests document references and the removal of secret values from audit records.

## 2. Applicability

- Use the candidate source checkout.
- On Linux, test local Chromium and Firefox.
- On macOS, test local Safari with Safari automation enabled.
- Safari and remote sessions do not support child-frame semantic input.
- This procedure does not test the Horizon UI or remote device providers.

## 3. Safety

> **CAUTION:** USE ONLY THE SYNTHETIC FORM VALUES. Do not use account credentials.

The HTTP fixture uses loopback servers and temporary browser profiles.
The test does not use the developer's browser session.

## 4. Equipment and preconditions

- Rust and the workspace build prerequisites.
- On Linux, local Chromium, Firefox and geckodriver on PATH.
- On macOS, local Safari and safaridriver with Safari automation enabled.
- Permission to bind loopback sockets.
- On Linux, permission to start headless browser processes.
- On macOS, permission to start a Safari automation session.

## 5. Setup

1. Open a shell in the candidate checkout.
   On Linux, if Firefox uses Snap, set `TMPDIR` to a private directory below the home directory.
   The directory must not have a name that starts with a dot.
   Firefox and geckodriver must both have access to it.
2. On Linux, run `cargo test -p horizon-browser-mcp --test cross_origin_frames_live -- --ignored --nocapture`.
3. On macOS, run `cargo test -p horizon-browser-mcp --test cross_origin_frames_live mcp_safari_preserves_top_level_input_and_frame_boundaries -- --ignored --nocapture`.
   Run the selected test alone. Each test uses its own coordination root.

   Result: The test starts an isolated MCP process for each browser session.

## 6. Tasks

On Linux, use tasks 6.1 through 6.6. On macOS, use tasks 6.6 and 6.7.

### 6.1 FRAME-INPUT — Fill and submit the form

1. Examine the completed test result.

   Result: `browser_snapshot` returns an input reference from the cross-origin frame.
   `browser_act` fills the user field through that reference.
   `browser_query` returns the password field and submit button.
   `browser_act` fills the password field and clicks the button.
   The HTTP fixture records a trusted form submission.
   The top-level input with the same ID retains its original value.
   Each backend repeats the flow with a nested frame.

### 6.2 FRAME-STALE — Replace the child document

1. Examine the completed test result.

   Result: The test reloads the child frame at the same URL.
   A fill with the previous reference fails with `stale_reference`.
   A new query returns a new valid reference.
   Each backend repeats the reload three times.
   A new query also invalidates a reference from the previous snapshot.
2. Run `cargo test -p horizon-browser multi_frame_scan_rejects_an_earlier_child_invalidated_during_a_later_scan`.

   Result: Each protocol fixture scans one child before it scans another child.
   The first child changes or is removed during the second scan.
   The scan returns `stale_reference` and does not publish the old child nodes.
   Existing top-level references remain valid.
   A scan with no document change returns both child nodes.
   Navigation, context removal and detach events for unrelated pages do not interrupt the scan.

### 6.3 FRAME-AUDIT — Examine the audit

1. Examine the completed test result.

   Result: `browser_audit` returns no user or password fill value.

### 6.4 FRAME-SIBLING — Check reference isolation

1. Examine the completed test result.

   Result: The test adds a sibling frame at the same URL.
   A query returns two password references.
   Each reference fills a different document.
   The test removes one sibling.
   Its reference becomes stale. The other reference remains usable.

### 6.5 FRAME-ERROR — Check bounds and rejected actions

1. Examine the completed test result.

   Result: Invalid selectors return `invalid_selector`.
   The node limit applies when the top document fills the limit and when child documents fill it.
   Hidden, disabled and non-editable targets return the applicable errors.
   Read-only inputs, text areas and ARIA read-only fields reject fill actions.
   Their existing values do not change in the child or top-level document.
   A field that becomes read-only on focus also retains its value.
   If an onfocus handler moves focus or an inert ancestor prevents focus, the fill returns `element_not_focused`.
   The rejected fill does not change the original value or send an input event.
   The test checks these cases in child and top-level documents.
   If an input handler moves focus during clearing, the requested text does not reach the field that receives focus.
   The handler transfers the target ID to the field that receives focus. The original element check still rejects the fill.
   The test also moves focus through queued and nested microtasks after focus and clearing.
   A queued onfocus redirect retains the original value and sends no input event.
   A queued input redirect sends no requested text or input event to the field that receives focus.
   File, checkbox, radio, range, button, color, date and select controls reject fill without value changes or input events.
   These controls also reject fill when they have `contenteditable="true"`.
   Text, search, tel, URL, email, password and number inputs accept fill.
   Editable text areas and contenteditable elements also accept fill.
   Child file inputs have no file-upload capability marker.
   A scroll action on a child reference returns `unsupported_frame_action`.

### 6.6 FRAME-CAPTURE — Check frame delivery

1. Examine the completed test result.

   Result: After the input tasks, the test changes the page background and fills a top-level field.
   A new decoded frame arrives within three seconds.

### 6.7 SAFARI-COMPAT — Check the supported Safari behavior

1. Examine the macOS test result.

   Result: The snapshot shows the iframe boundary.
   A query returns no child password input.
   Top-level reference input works and its value is absent from the audit.
   Read-only top-level controls retain their values after rejected fill actions.
   A field rejected because of onfocus redirection or an inert ancestor retains its value and receives no input event.
   If focus moves during the clearing event, the requested text does not reach the field that receives focus.
   This result does not qualify child-frame input on Safari.

## 7. Pass criteria

- On Linux, the Chromium and Firefox assertions in tasks 6.1 through 6.6 pass.
- On macOS, the Safari assertions in tasks 6.6 and 6.7 pass.
- The selected platform tasks pass through public MCP tools.
- Decoded frame delivery continues after the input tasks.
- The test does not require a human handoff.

## 8. Cleanup

1. Wait for the test process to exit.

   Result: The test stops its MCP processes, browser sessions and HTTP fixture.
   Temporary profiles and coordination state are removed.
   The same cleanup runs if a test assertion fails.

## 9. Record of results

Record the candidate commit, command, browser versions and result in the pull request.
Keep private paths and machine information out of the pull request.
