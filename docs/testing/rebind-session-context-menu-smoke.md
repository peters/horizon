# macOS Rebind Session Context Menu Smoke

## Setup

- Build the debug binary on macOS: `cargo build -p horizon-ui`.
- Use an isolated home directory so local settings and real session state are not touched.
- Create an isolated Horizon config with one workspace and one binding-capable agent panel whose working directory matches the seeded session catalog.
- Seed at least two recent local agent session records for that same agent kind and working directory.
- Launch `target/debug/horizon` with the isolated `HOME`.

## Baseline

- Confirm the root Horizon window opens and shows the seeded workspace and panel.
- Confirm the panel titlebar responds to left-click focus.
- Right-click or Control-click the panel titlebar and confirm the context menu opens.
- Capture a baseline screenshot with `screencapture -x <path>`.

## Primary Flow

- Open the panel titlebar context menu.
- Confirm each available rebind candidate appears as a visible top-level row prefixed with `Rebind Session`.
- Click a rebind row directly and confirm the menu closes immediately.
- Quit Horizon cleanly.
- Inspect the isolated runtime state and confirm the panel `resume` value is `Session` with the clicked session id.
- Confirm the panel `session_binding.session_id` matches the same clicked session id.

## Edge Cases

- Repeat with only one available rebind option and confirm the single visible row is clickable.
- Repeat when the current binding is already the newest matching session and confirm the current session is omitted from the menu.
- Repeat with a non-agent panel and confirm no rebind rows appear.
- Repeat with an agent kind that does not support exact session binding and confirm no rebind rows appear.
- Reopen the context menu after a successful rebind and confirm the newly selected session is no longer listed as a candidate.

## Persistence

- Relaunch Horizon with the same isolated `HOME`.
- Confirm the panel restores with the rebound `Session` resume target.
- Restart the rebound panel.
- Confirm the launched agent command receives the selected session id.

## Visual Checks

- Capture a screenshot immediately after opening the panel context menu.
- Confirm the menu width expands enough for the rebind rows without clipping or overlapping the workspace and restart actions.
- Resize the Horizon window and reopen the same context menu near the left, right, top, and bottom edges of the viewport.
- Confirm the menu remains usable and does not overlap panel chrome in a way that blocks clicking a visible rebind row.
