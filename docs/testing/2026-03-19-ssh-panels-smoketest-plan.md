# SSH Panels Smoke-Test Plan

Execution status: not executed on this machine.

## Goal

Validate Horizon issue #51 end to end:

- `kind: ssh` presets defined in Horizon config work.
- `~/.ssh/config` host discovery populates SSH preset entries in the command palette.
- SSH panels launch without requiring a local workspace directory.
- SSH panel chrome shows the SSH visual treatment and status badge.
- SSH panels stay visible after disconnect and can be manually reconnected.
- SSH connection metadata persists through runtime save/load.

## Scope

This smoke test covers:

- Config-defined SSH presets
- Discovered SSH presets from `~/.ssh/config`
- Panel creation from command palette and canvas preset picker
- Terminal interaction, resize, focus, and close/reconnect behavior
- Runtime/session persistence
- Visual regression checks for title bar, sidebar, and palette

This smoke test does not cover:

- Automatic reconnect backoff
- Exit-code-based differentiation between intentional logout and transport failure
- Non-OpenSSH clients

## Preconditions

Use a separate test machine or disposable VM/container. Do not run this plan in the authoring environment.

Requirements:

- Rust toolchain and system dependencies needed to run Horizon
- OpenSSH client available as `ssh`
- At least one reachable SSH target
- Preferably two targets:
  - `loopback`: an SSH server on `127.0.0.1` or the same machine
  - `remote`: a second reachable host or VM
- Ability to edit:
  - `~/.ssh/config`
  - `~/.horizon/config.yaml`
- A clean checkout of the issue branch worktree

Recommended evidence capture:

- Screenshots
- Terminal logs
- Saved runtime YAML before and after restart

## Test Data Setup

### 1. Prepare SSH config discovery fixtures

Back up any existing `~/.ssh/config`, then create a test-focused config containing all of these cases:

```sshconfig
Host local-smoke
  HostName 127.0.0.1
  User smoke
  Port 22

Host remote-smoke bastion-smoke
  User smoke
  Port 2222
  ProxyJump jumpbox.example.test

Host command-smoke
  HostName 127.0.0.1
  User smoke
  RemoteCommand tmux new -A -s horizon-smoke

Host *
  User should-not-create-a-preset

Host *.wildcard.test
  User should-also-be-skipped
```

Expected discovery behavior:

- `local-smoke` appears
- `remote-smoke` appears
- `bastion-smoke` appears
- `command-smoke` appears
- `Host *` does not become a preset
- `Host *.wildcard.test` does not become a preset

### 2. Prepare Horizon config fixtures

Back up `~/.horizon/config.yaml`, then add these presets:

```yaml
presets:
  - name: "SSH: Config Prod"
    alias: "scp"
    kind: ssh
    ssh_connection:
      host: local-smoke
      user: smoke
      port: 22

  - name: "SSH: Command Host"
    alias: "sch"
    kind: ssh
    ssh_connection:
      host: command-smoke
      user: smoke

  - name: "SSH: Fallback Command"
    alias: "sfc"
    kind: ssh
    command: ssh
    args:
      - local-smoke
```

Keep at least one non-SSH preset in the file to verify mixed lists still work.

### 3. Build the binary

From the issue worktree:

```bash
cargo build
```

If the test machine supports GPU-backed launch, prefer running the normal app binary instead of headless checks.

## Smoke-Test Matrix

Run all rows:

| Area | Scenario | Expected |
|---|---|---|
| Discovery | `~/.ssh/config` hosts appear | SSH presets visible in palette |
| Config presets | YAML `kind: ssh` preset opens panel | Working SSH terminal |
| Palette | Search by alias/user/host | Matching SSH presets returned |
| Workspace flow | New SSH panel in workspace with no cwd | No directory prompt |
| New workspace flow | New SSH workspace from preset picker | No directory prompt |
| Chrome | SSH title bar tint/icon/status | Visually distinct and readable |
| Disconnect | Remote shell exits | Panel remains visible, status becomes disconnected |
| Reconnect | Context menu reconnect | Panel relaunches and connects again |
| Persistence | Save/load runtime state | `kind: ssh` and `ssh_connection` preserved |
| Regression | Shell/editor/agent presets still work | No behavior regression |

## Primary Flow

### 1. Launch Horizon

Run the app:

```bash
cargo run
```

Expected:

- App window opens normally
- Existing non-SSH workspaces/panels still render correctly
- No startup panic related to config parsing or preset loading

Capture:

- Screenshot of initial launch

### 2. Verify command-palette SSH entries

Open the command palette with `Ctrl+K`.

Checks:

- Discovered SSH entries appear with labels beginning `SSH:`
- Config-defined SSH presets appear alongside normal presets/actions
- Wildcard hosts do not appear
- Multiple aliases from one `Host` line appear as separate entries

Search cases:

- Search by preset name: `config prod`
- Search by host alias: `local-smoke`
- Search by SSH user: `smoke`
- Search by alias shortcut text where relevant: `scp`

Expected:

- Matching SSH entries are returned
- Palette remains responsive
- No duplicate result rows for the same discovered host

Capture:

- Screenshot of the palette with at least one discovered SSH entry and one config-defined SSH entry visible

### 3. Open an SSH panel from the command palette

Select the discovered `SSH: local-smoke` entry.

Expected:

- A panel is created in the active workspace
- Horizon does not open the directory picker
- Panel uses terminal rendering, not editor/static rendering
- Panel title keeps the SSH host visible
- Status badge starts as `Connecting...`
- Status becomes `Connected` after remote output arrives

Interact with the session:

- Run `pwd`
- Run `echo HORIZON_SSH_SMOKE`
- Resize the panel
- Scroll back
- Focus another panel and return

Expected:

- Input/output work normally
- Resize updates rows/cols without breaking the session
- Scrollback behaves like shell panels

Capture:

- Screenshot of a connected SSH panel after running a command

### 4. Open an SSH panel from a config-defined preset

Use the palette entry for `SSH: Config Prod`.

Expected:

- Same behavior as the discovered preset
- Structured config values are honored
- No directory picker appears

### 5. Verify fallback command launch path

Open `SSH: Fallback Command`.

Expected:

- Panel launches successfully even without `ssh_connection`
- Behavior matches a direct `ssh local-smoke` command launch

### 6. Create an SSH panel in a workspace that has no local cwd

Create a brand new empty workspace with no selected directory, then add an SSH preset.

Expected:

- No directory picker appears for the SSH preset
- Panel is created successfully in the empty workspace

### 7. Create a new workspace from the preset picker

Use the canvas preset picker path, not only the command palette:

- Trigger the canvas preset picker
- Choose an SSH preset while there is no target workspace under the pointer

Expected:

- Horizon creates a new workspace directly
- No directory picker appears
- SSH panel opens in that new workspace

## Chrome And Visual Checks

For an SSH panel, verify all of the following:

- Title bar tint is visually distinct from ordinary shell panels
- Sidebar icon is `SSH`
- Title always shows the SSH host label
- If the panel has a custom name, the host label still remains visible
- Status badge is readable in all three states:
  - `Connecting...`
  - `Connected`
  - `Disconnected`

Visual regression checks:

- Panel width near the minimum supported width does not cause title/status overlap severe enough to hide all context
- Sidebar row truncation still leaves enough of the SSH host visible to identify the panel
- Command palette rows do not clip discovered SSH entries

Capture:

- Screenshot of a narrow SSH panel
- Screenshot of the sidebar with at least one SSH panel entry

## Disconnect And Reconnect

### 1. Intentional disconnect

Inside the SSH session, run `exit`.

Expected:

- The panel remains on the canvas
- The terminal content remains visible
- Status changes to `Disconnected`
- Horizon does not silently remove the panel

### 2. Manual reconnect

Use the panel context menu and the sidebar context menu.

Checks:

- Right-click the panel title bar and use `Reconnect`
- Right-click the sidebar entry and use `Reconnect`

Expected:

- The panel relaunches in place
- Layout, panel identity, and workspace assignment are preserved
- Status returns to `Connecting...`, then `Connected`

Capture:

- Screenshot of a disconnected SSH panel
- Screenshot after reconnect completes

## Persistence And Migration

### 1. Save runtime state with live SSH panels

With at least two SSH panels open:

- One discovered preset
- One config-defined preset

Trigger normal app shutdown so runtime state is saved.

Inspect the saved runtime/session YAML.

Expected serialized fields:

- `kind: ssh`
- `ssh_connection.host`
- Optional structured SSH fields when configured:
  - `user`
  - `port`
  - `identity_file`
  - `proxy_jump`
  - `remote_command`

Expected:

- SSH metadata is present
- Runtime save does not collapse SSH panels into plain shell panels

### 2. Reopen Horizon

Relaunch the app and restore the saved session.

Expected:

- SSH panels are restored as SSH panels
- Chrome still uses SSH tint/icon/status badge
- The host label remains visible
- Reconnect remains available if a restored SSH panel is disconnected

## Edge Cases

### 1. Missing `~/.ssh/config`

Temporarily move the SSH config file out of the way and relaunch.

Expected:

- Horizon still launches
- No discovered SSH presets appear
- Config-defined SSH presets still appear

### 2. Empty or invalid `ssh_connection.host`

Set a config preset to:

```yaml
kind: ssh
ssh_connection:
  host: ""
```

Expected:

- Config load fails with a clear validation error
- Error points to the SSH connection host requirement

### 3. Duplicate discovered/config preset names

Create a config-defined preset with the same name as a discovered SSH preset.

Expected:

- The user-defined preset wins
- Horizon does not show a duplicate discovered entry with the same label

### 4. Search behavior with multiple aliases

For `Host remote-smoke bastion-smoke`, verify:

- Both entries appear
- Each entry launches successfully
- Search finds the correct alias when typed exactly

## Regression Checks

Verify these non-SSH paths after the SSH changes:

- Create a normal shell panel
- Create a Codex panel
- Create a Claude panel
- Create a Markdown panel
- Create a Git Changes panel
- Open the command palette and confirm ordinary actions still execute

Expected:

- No regressions in non-SSH panel creation
- Command palette categories still behave correctly
- Non-SSH panels do not show SSH chrome/status

## Pass/Fail Criteria

Pass when all of the following are true:

- SSH presets can be created from both config and discovery
- SSH panel creation does not require a directory prompt
- SSH panels show distinct chrome and meaningful status
- Disconnect leaves the panel visible
- Reconnect works from both panel and sidebar context menus
- Runtime state preserves `kind: ssh` and `ssh_connection`
- No obvious regression in existing preset and palette flows

Fail on any of the following:

- SSH panel closes immediately on disconnect
- SSH entries never appear in the command palette
- Host label disappears from the title bar
- Reconnect action is missing or broken
- Persistence reload turns SSH panels into plain shells
- Palette or chrome layout visibly overlaps or clips essential SSH labels

## Cleanup

After the smoke test:

- Restore the original `~/.ssh/config`
- Restore the original `~/.horizon/config.yaml`
- Remove any temporary SSH targets or loopback test accounts
- Archive screenshots and logs with the test report
