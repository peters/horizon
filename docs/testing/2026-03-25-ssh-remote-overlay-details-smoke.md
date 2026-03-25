# SSH Remote Overlay Expand/Collapse Smoke Plan

Status: planned only. Not executed on this machine.

Branch: `feat/ssh-remote-overlay-details`

## Goal

Validate the SSH remote overlay changes that add:

- Expand/collapse per remote host row
- Current SSH session status derived from live/disconnected SSH panels
- Connection history preview per host

## Environment

- Run from the exact branch/commit under review
- Use `target/debug/horizon`
- Use a test account with a populated `~/.ssh/config`
- Prefer a setup that has:
  - At least 3 discovered SSH hosts
  - At least 1 host with Tailscale metadata
  - At least 1 host that has no prior SSH panel history
  - At least 1 currently connected SSH panel
  - At least 1 disconnected SSH snapshot/history entry

## Pre-flight Setup

1. Build the app:
   ```bash
   cargo build
   ```
2. Confirm the branch is clean except for the feature under review.
3. If needed, seed history by opening and closing SSH panels for at least two distinct hosts.
4. Make note of one host that is currently connected and one host with only disconnected history.

## Baseline Launch

1. Launch Horizon:
   ```bash
   target/debug/horizon
   ```
2. Open the SSH remote overlay from the normal shortcut or toolbar action.
3. Verify the overlay appears centered, the query field is focused, and host rows render without clipping.
4. Capture one screenshot of the collapsed overlay state.

## Primary Interaction Checks

1. Click the expand chevron on a host with known SSH history.
2. Verify only that row expands and the overlay remains scrollable.
3. Verify the expanded card shows:
   - Current SSH session status badge
   - Session counts (`live` and `total`)
   - Source/network metadata badges
   - Connection history list
4. Verify a second click on the same chevron collapses the detail card.
5. Expand a different host and verify the previous host collapses.
6. Single-click a non-selected row body and verify it only changes selection.
7. Click the selected row body again and verify it opens the SSH panel.
8. Double-click an unselected row body and verify it opens the SSH panel directly.

## Status Coverage

1. For a host with a live SSH panel, verify the expanded status badge reports `Connected` or `Connecting` as appropriate.
2. For a host with only prior/disconnected panels, verify the expanded status badge reports `Disconnected`.
3. For a host with no prior SSH panels, verify the expanded state renders and clearly shows that no sessions have been opened from this board yet.
4. If multiple SSH panels exist for one host, verify the `live / total` counts match the visible panel set.

## History Coverage

1. Expand a host with multiple prior sessions.
2. Verify the history list is ordered newest first.
3. Verify each history entry includes:
   - Relative age
   - Panel title
   - Workspace name
4. If more than four sessions exist, verify the `+N older session(s)` summary appears.

## Filtering And Selection

1. Type a filter that matches a host with history and verify expand/collapse still works.
2. Type a filter that matches a host without history and verify the empty-history message is correct.
3. Clear the filter and verify selection resets to the first visible row.
4. Use `ArrowUp`/`ArrowDown` to move selection and `Enter` to connect.
5. Press `Escape` to close the overlay.

## Persistence / Relaunch

1. Open an SSH panel from the overlay.
2. Wait until the panel is visibly connected, then close or disconnect it so Horizon retains the SSH snapshot.
3. Reopen the overlay and verify the same host now shows the new history entry.
4. Fully quit Horizon and relaunch into the same runtime state.
5. Reopen the overlay and verify disconnected SSH snapshots still contribute to the host history/status summary.

## Visual Regression Checks

1. Verify expanded cards do not overlap adjacent rows or overflow the overlay card.
2. Verify long host aliases, workspace names, and panel titles remain readable or truncate cleanly.
3. Verify collapsed rows still align with the existing column headers.
4. Verify the chevron affordance is visible for both hosts with and without history.
5. Resize the main window smaller and larger while the overlay is open and verify the expanded card reflows without clipping.
6. Capture a second screenshot with one row expanded.

## Failure Notes To Record

- Any mismatch between visible SSH panels and the reported `live / total` counts
- Any host row where expand/collapse changes the wrong host
- Any history ordering issue
- Any overlap, clipping, or scroll jitter in the expanded overlay
- Any case where connect-on-click stops working for the selected row

## Evidence To Attach To PR

- One screenshot of the collapsed overlay
- One screenshot of an expanded host
- Short note stating which hosts covered:
  - live session
  - disconnected history
  - no-history case
