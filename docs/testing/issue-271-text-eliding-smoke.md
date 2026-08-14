# Issue 271 macOS text eliding and tooltip smoke plan

## Purpose

Validate the exact issue-271 PR head on macOS/Metal after the shared truncation,
single-line layout, tooltip, and badge-painting changes. Use an isolated Horizon
config/session state and target only the process launched for this test.

## Evidence to capture

- Exact PR head SHA, macOS version, CPU architecture, display scale, and Metal
  adapter/backend summary.
- Launch screenshot and a screenshot after resizing the root window.
- Before/after screenshots for every tooltip or label lane exercised.
- For a continuously open long tooltip, a 5-second video or screenshots at
  0, 2, and 5 seconds proving its width does not shrink.
- Any crash log, rendering warning, or visual discrepancy.

## Setup

1. Check out the PR branch, run `git lfs pull`, and confirm `git rev-parse HEAD`
   equals the requested PR head SHA.
2. Build `target/debug/horizon` from that exact head. Prefer a speech-enabled
   build for the microphone-tooltip lane when its macOS prerequisites are
   available; record any lane that is unavailable.
3. Create a temporary working directory outside the repository. From that
   directory, launch `target/debug/horizon --ephemeral --blank` with `HOME`
   unset so Horizon uses only the temporary relative `.horizon` state. Do not
   reuse or automate the daily session.
4. Record the exact test PID and identify its root and detached windows by PID,
   not by application name alone.
5. Confirm the adapter summary reports Metal, and run on a Retina-scaled display
   when available.

## Baseline launch and resize

1. Launch with a clean state and confirm a visible, responsive root window.
2. Capture the launch screenshot.
3. Resize narrower and then wider. Confirm panel chrome, workspace labels,
   minimap labels, and toolbar controls remain visible without overlap.
4. Capture the post-resize screenshot.

## Unicode truncation and badge rendering

1. Exercise an attention summary containing the boundary string
   `aaaaaaaaaaaaaaaaaaaaaaaaaaaaåzz`, Norwegian text, and emoji.
2. Confirm no crash occurs and the summary ends with a single Unicode
   ellipsis within the badge.
3. Exercise an SSH upload filename longer than 20 characters containing `å`,
   emoji, or both; confirm a single `…` is shown rather than `...`.
4. Populate a long Unicode terminal search result and confirm its detail text
   truncates without malformed characters.
5. Compare attention, minimap-label, search-count, and command-shortcut badge
   backgrounds in dark and light themes. Their previous corner radii, fills,
   strokes, alignment, and clipping must remain visually unchanged.

## Single-line title and workspace labels

1. Use long panel and workspace names at wide and narrow widths.
2. Include embedded newlines and confirm the full source is represented on one
   elided line rather than stopping at the first newline.
3. Confirm vertical centering, grip spacing, titlebar controls, and minimap
   label placement remain unchanged.

## Stable tooltip lanes

For each available lane below, keep the pointer still over the target for at
least five seconds while Horizon redraws. Confirm one single-line tooltip stays
inside the viewport, uses `…` when needed, never narrows frame over frame, and
does not spawn a nested second tooltip.

- Toolbar FPS meter, including its idle state.
- Seeded update tooltip with newlines and a long download error.
- Panel microphone tooltip with a long speech-hotkey summary.
- Invalid shortcut error indicator.
- Empty preset-name validation.
- Detached-window Fit Workspace and minimap shortcut buttons.
- Minimap target with a long Unicode workspace/panel label.

Repeat the long-tooltip checks in a narrow root viewport and, where applicable,
a narrow detached viewport. For at least one long tooltip, capture screenshots
at 0, 2, and 5 seconds with a stationary pointer to prove its width does not
shrink across Metal redraws.

## Interaction, persistence, and migration

1. Click controls after their tooltips have been open to confirm tooltip
   attachment did not consume or duplicate the interaction response.
2. Rename a workspace and panel, restart with the isolated state, and confirm
   names persist and still elide correctly.
3. Confirm existing state/config files load without a migration prompt or
   rewrite; this change introduces no persisted fields.
4. Verify minimap accessibility text still names the hovered target.

## Pass criteria

- No panic, malformed UTF-8, tooltip shrink-collapse, nested tooltip, badge
  style regression, clipping regression, or broken click behavior.
- Launch and resize screenshots show a fully rendered UI.
- Automated and manual observations were made on the exact requested PR head.

Post the results to the PR as:

```text
SMOKE-TEST REPORT (macOS)
- <step>: pass | fail — short note
- ...
Summary: <fixes pushed, remaining issues, and evidence locations>
SMOKE-TEST: DONE
```

After every lane passes, delete this temporary plan, push that cleanup to the
same PR branch, and include the resulting head SHA in the report. The final line
must be exactly `SMOKE-TEST: DONE`.
