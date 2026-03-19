# Structured Navigation Smoke Test

Validates the shortcuts added for issue #15: Focus Workspace, Next/Previous
Workspace cycling, and their command palette integration.

## Prerequisites

- Build from this branch (`cargo run --release`)
- At least two workspaces with panels (default config or manual creation)
- Sidebar visible (Ctrl+B)

## 1. Focus Active Workspace (Ctrl+Shift+F)

| Step | Action | Expected |
|------|--------|----------|
| 1.1 | Pan far away from all workspaces | Canvas shows empty area |
| 1.2 | Press Ctrl+Shift+F | Canvas pans and centers on the active workspace |
| 1.3 | Zoom out to minimum zoom | Workspace is very small |
| 1.4 | Press Ctrl+Shift+F | Canvas pans to center the active workspace (zoom unchanged) |
| 1.5 | Click a panel in a different workspace via sidebar | That workspace becomes active |
| 1.6 | Press Ctrl+Shift+F | Canvas centers on the newly active workspace |

## 2. Next Workspace (Ctrl+Shift+Right)

| Step | Action | Expected |
|------|--------|----------|
| 2.1 | Note which workspace is active in sidebar | Active workspace highlighted |
| 2.2 | Press Ctrl+Shift+Right | Next workspace becomes active, canvas pans to it |
| 2.3 | Press Ctrl+Shift+Right again | Cycles to the following workspace |
| 2.4 | Keep pressing until past the last workspace | Wraps back to the first workspace |

## 3. Previous Workspace (Ctrl+Shift+Left)

| Step | Action | Expected |
|------|--------|----------|
| 3.1 | Press Ctrl+Shift+Left | Previous workspace becomes active, canvas pans to it |
| 3.2 | Keep pressing until past the first workspace | Wraps to the last workspace |

## 4. Interaction with Focused Terminal

| Step | Action | Expected |
|------|--------|----------|
| 4.1 | Click a terminal panel to focus it | Terminal accepts keyboard input |
| 4.2 | Press Ctrl+Shift+F | Focus workspace fires (shortcut works even with terminal focus) |
| 4.3 | Press Ctrl+Shift+Right | Next workspace fires |
| 4.4 | Press Ctrl+Shift+Left | Previous workspace fires |

## 5. Command Palette Integration

| Step | Action | Expected |
|------|--------|----------|
| 5.1 | Press Ctrl+K | Command palette opens |
| 5.2 | Type "focus" | "Focus Workspace" appears with Ctrl+Shift+F shortcut shown |
| 5.3 | Select "Focus Workspace" | Palette closes, canvas centers on active workspace |
| 5.4 | Press Ctrl+K, type "next" | "Next Workspace" appears with shortcut |
| 5.5 | Press Ctrl+K, type "prev" | "Previous Workspace" appears with shortcut |

## 6. Single Workspace Edge Case

| Step | Action | Expected |
|------|--------|----------|
| 6.1 | Close all workspaces except one | Only one workspace remains |
| 6.2 | Press Ctrl+Shift+Right | Active workspace does not change (stays on same) |
| 6.3 | Press Ctrl+Shift+Left | Same - no crash, no change |
| 6.4 | Press Ctrl+Shift+F | Canvas centers on the single workspace |

## 7. Detached Workspace

| Step | Action | Expected |
|------|--------|----------|
| 7.1 | Right-click a workspace in sidebar, "Open in New Window" | Workspace detaches |
| 7.2 | Press Ctrl+Shift+Right | Skips the detached workspace |
| 7.3 | Press Ctrl+Shift+F | Only focuses attached workspaces |
