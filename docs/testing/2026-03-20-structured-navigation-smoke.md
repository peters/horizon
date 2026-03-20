# Structured Navigation Smoke Test

Use this plan to validate issue `#15` on branch `feat/onboarding-structured-navigation`.
Delete this file after the smoke pass is complete unless the user explicitly wants it kept.

## Goal

Verify that Horizon now surfaces a structured workflow for users who do not want to start with drag-heavy canvas interaction:

- toolbar exposes workspace-first navigation affordances
- empty state explains and exposes the workflow
- explicit focus/fit workspace actions work
- quick-nav, presets, and workspace layout controls remain discoverable

## Setup

1. Use the checkout at `/home/peters/github/horizon-structured-nav`.
2. Run `cargo run --release`.
3. Test on the exact branch above, not a disposable merge checkout.
4. Use the updated local config at `~/.horizon/config.yaml`.

## Evidence To Capture

- One screenshot right after launch on the empty state.
- One screenshot after creating content and triggering **Fit Workspace**.
- If any interaction behaves unexpectedly, capture the failing state before closing the app.

## Smoke Scenarios

### 1. Empty State

- Launch Horizon with no restored panels.
- Confirm the empty-state card is visible in the canvas.
- Confirm the card includes buttons for:
  - `New Workspace`
  - `New Terminal`
  - `Quick Nav`
  - `Fit Workspace`
- Confirm the card text mentions:
  - workspace-first flow
  - preset-driven terminals
  - Quick Nav
  - Rows / Cols / Grid workspace layout controls
- Confirm `Fit Workspace` is disabled when no attached workspace exists.

### 2. Toolbar Discoverability

- Confirm the top toolbar includes:
  - `New Workspace`
  - `Quick Nav`
  - `Fit Workspace`
  - `Remote Hosts`
  - `Settings`
- Hover `Quick Nav` and confirm the tooltip matches the configured shortcut.
- Hover `Fit Workspace` and confirm the tooltip matches the configured shortcut.

### 3. Workspace Creation Flow

- Click `New Workspace`.
- Confirm a new workspace is created and becomes visible without dragging.
- Confirm the empty-state card disappears once a panel is created later.

### 4. Preset-Led Terminal Creation

- With at least one workspace present, click `New Terminal`.
- Confirm Horizon creates a panel from the first preset instead of failing.
- Confirm the panel appears in the active workspace.

### 5. Fit Workspace

- Resize or move the panel so the workspace does not already fill the view.
- Trigger `Fit Workspace` from the toolbar.
- Confirm the active workspace is fully framed inside the visible canvas.
- Confirm the fit action changes zoom when needed.
- Capture the post-fit screenshot here.

### 6. Quick Nav And Commands

- Open `Quick Nav` from the toolbar.
- Confirm action results include:
  - `Focus Active Workspace`
  - `Fit Active Workspace`
- Execute `Focus Active Workspace`.
- Confirm the current workspace is centered/refocused without changing zoom.
- Execute `Fit Active Workspace`.
- Confirm the current workspace is fully framed.

### 7. Workspace Context Menu

- Open the context menu on a workspace label.
- Confirm the menu includes:
  - `Focus Workspace`
  - `Fit Workspace`
  - existing layout actions
- Run `Focus Workspace` and confirm the workspace is brought into focus.
- Run `Fit Workspace` and confirm the workspace is fully framed.

### 8. Layout Discoverability

- With a workspace containing panels, confirm the workspace header still exposes `Default`, `Rows`, `Cols`, and `Grid`.
- Apply at least one layout mode.
- Confirm layout controls still work after using focus/fit actions.

### 9. Regression Checks

- `Ctrl+Shift+K` still opens Quick Nav.
- `Ctrl+Shift+N` still creates a terminal.
- `Ctrl+Shift+W` focuses the active workspace.
- `Ctrl+Shift+9` fits the active workspace.
- The sidebar still lists workspaces and panels correctly.
- Remote Hosts and Settings buttons still work from the toolbar.

## Pass Criteria

- Structured-navigation controls are visible without needing drag-first discovery.
- Focus and fit workspace actions work from both toolbar/palette and workspace context menu.
- No obvious UI overlap or toolbar breakage on a normal desktop-sized window.
- Launch and post-fit screenshots are captured successfully.
