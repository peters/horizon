# macOS Automatic Workspace Organization Startup Smoke Test

## Scope

Validate on macOS that Horizon automatically performs the same attached-workspace organization as
`Cmd+Shift+A` exactly once after each session finishes loading when the opt-in setting is enabled,
including initial startup, relaunch, and in-process session switches. Also verify that a missing or
disabled setting preserves the restored layout. Run every step from the exact pull-request head and
use `target/debug/horizon`.

## Environment and exact-head guard

Record the macOS version, CPU architecture, display/Retina setup, `rustc --version`, and
`cargo --version`. Before building, record `git rev-parse HEAD` and verify it equals the pull request
head. Repeat that comparison immediately before posting the report. Stop and restart this plan if the
head changes.

Build the exact checkout:

```bash
cargo build
```

Create an isolated persistent home outside the repository. Keep it until the report is accepted so
the original config and generated runtime files remain available as evidence:

```bash
export SMOKE_ROOT="$(mktemp -d /tmp/horizon-auto-organize-enabled.XXXXXX)"
export SMOKE_DEFAULT_ROOT="$(mktemp -d /tmp/horizon-auto-organize-disabled.XXXXXX)"
mkdir -p "$SMOKE_ROOT/.horizon" "$SMOKE_ROOT/config" "$SMOKE_ROOT/data" "$SMOKE_ROOT/cache"
mkdir -p "$SMOKE_DEFAULT_ROOT/.horizon" "$SMOKE_DEFAULT_ROOT/config" "$SMOKE_DEFAULT_ROOT/data" "$SMOKE_DEFAULT_ROOT/cache"
cat > "$SMOKE_ROOT/.horizon/config.yaml" <<'YAML'
version: 8
window:
  width: 1400
  height: 900
appearance:
  theme: dark
features:
  attention_feed: false
  organize_workspaces_on_session_load: true
workspaces:
  - name: Alpha Smoke
    position: [100, 300]
    terminals:
      - name: Alpha Notes
        kind: editor
        position: [20, 60]
        size: [320, 220]
  - name: Beta Smoke
    position: [650, 520]
    terminals:
      - name: Beta Notes
        kind: editor
        position: [20, 60]
        size: [320, 220]
  - name: Gamma Smoke
    position: [1200, 80]
    terminals:
      - name: Gamma Notes
        kind: editor
        position: [20, 60]
        size: [320, 220]
YAML

awk '!/^[[:space:]]+organize_workspaces_on_session_load:/' \
  "$SMOKE_ROOT/.horizon/config.yaml" \
  > "$SMOKE_DEFAULT_ROOT/.horizon/config.yaml"
```

Use the matching root for every `HOME`, XDG directory, and config argument. Keep
`RUST_LOG=horizon=debug,horizon_core=info` so the structured startup-organization outcome is present
in captured logs. For relaunch steps, omit `--new-session` and select the saved session when
prompted. Record the exact runtime path for each root rather than relying on a broad mixed-session
search.

## Default-disabled compatibility

Run this lane first. The field was omitted from the isolated `$SMOKE_DEFAULT_ROOT` config above; this
is the backward-compatibility case for existing config files and must behave the same as explicit
`false`:

```bash
HOME="$SMOKE_DEFAULT_ROOT" \
XDG_CONFIG_HOME="$SMOKE_DEFAULT_ROOT/config" \
XDG_DATA_HOME="$SMOKE_DEFAULT_ROOT/data" \
XDG_CACHE_HOME="$SMOKE_DEFAULT_ROOT/cache" \
RUST_LOG=horizon=debug,horizon_core=info \
target/debug/horizon --config "$SMOKE_DEFAULT_ROOT/.horizon/config.yaml" --new-session &
export HORIZON_DEFAULT_DISABLED_PID=$!
```

1. Resolve the root window by `$HORIZON_DEFAULT_DISABLED_PID` and do not invoke a layout command.
2. Verify `Alpha Smoke`, `Beta Smoke`, and `Gamma Smoke` retain y coordinates `300`, `520`, and `80`
   respectively after startup and after the runtime-state save debounce.
3. Open Settings → General → Features and verify **Organize Workspaces on Session Load** is unchecked.
4. Verify no modal **Preparing session view…** overlay is shown on this default-disabled path.
5. Record exactly one path as `DEFAULT_RUNTIME_PATH` with
   `find "$SMOKE_DEFAULT_ROOT/.horizon/sessions" -name runtime.yaml -print`, then close Horizon
   normally and verify the exact PID exits. Keep this isolated root as evidence.

## Cold-start organization

1. Only after the default-disabled PID has exited, launch the opt-in process:

   ```bash
   HOME="$SMOKE_ROOT" \
   XDG_CONFIG_HOME="$SMOKE_ROOT/config" \
   XDG_DATA_HOME="$SMOKE_ROOT/data" \
   XDG_CACHE_HOME="$SMOKE_ROOT/cache" \
   RUST_LOG=horizon=debug,horizon_core=info \
   target/debug/horizon --config "$SMOKE_ROOT/.horizon/config.yaml" --new-session &
   export HORIZON_SMOKE_PID=$!
   ```

2. Do not press `Cmd+Shift+A` or use the command palette.
3. Resolve the root window by `$HORIZON_SMOKE_PID`, not by application name, and verify that it is
   visible and responsive. Record the PID and native window inventory.
4. Verify the short **Preparing session view…** overlay blocks root-window keyboard, pointer, pan,
   zoom, fullscreen, file-drop, and session actions until the restored geometry settles, then disappears.
5. Wait through the runtime-state save debounce, record exactly one path as `RUNTIME_PATH` with
   `find "$SMOKE_ROOT/.horizon/sessions" -name runtime.yaml -print`, then inspect that file.
6. Verify all three attached workspace frames retain their original left-to-right order, share one y
   coordinate, and have non-overlapping x coordinates.
7. Capture a screenshot of the live root window. Check workspace frames, panels, names, sidebar,
   minimap, and canvas rendering on the Retina/Metal path.
8. Focus `Gamma Notes` in the non-leftmost `Gamma Smoke` workspace and wait for persistence. Move
   that workspace vertically, relaunch, and verify `Gamma Notes` and `Gamma Smoke` remain focused and
   active while startup alignment restores the row.

## One-shot and manual-command parity

1. Drag the middle workspace vertically away from the row.
2. Move the pointer over empty canvas and panel chrome for several seconds.
3. Verify the workspace does not snap back and its new position is persisted in `runtime.yaml`.
4. Press `Cmd+Shift+A` and verify the same three workspaces return to one horizontal row without
   changing the focused panel or active workspace.
5. If any snap-back, jitter, or oscillation is suspected, capture a short video with
   `screencapture -V` or a high-frequency native position trace scoped to the exact PID.

## Settings toggle and next-session-load behavior

1. Open Settings → General → Features and verify **Organize Workspaces on Session Load** is checked from
   the opt-in YAML.
2. Drag `Beta Smoke` vertically away from the row and wait for the new position to persist.
3. Uncheck **Organize Workspaces on Session Load**. Verify live preview leaves the board at its current
   positions, save, and confirm the YAML contains `organize_workspaces_on_session_load: false`.
4. Use the Sessions UI to create or load a throwaway session, then load the smoke session again
   without restarting Horizon. Verify `Beta Smoke` remains vertically offset. This proves explicit
   `false` does not alter the restored layout when `apply_runtime_state` loads another session.
5. Recheck the setting and save. Verify the live board remains offset and the YAML contains
   `organize_workspaces_on_session_load: true`; enabling the session-load action must not rearrange a running
   session.
6. Switch to the throwaway session and back to the smoke session again. Verify the three attached
   workspaces now form one horizontal row as soon as the session finishes loading.
7. Drag one workspace vertically, close Horizon normally, and relaunch the smoke session. Verify the
   startup load also restores the horizontal row. Record before/after runtime positions and keep a
   screenshot from both the disabled and enabled session loads.

## Persistence and relaunch

1. Drag one workspace out of the row again and wait for persistence.
2. Close Horizon normally. Verify the exact PID exits and its session lease is removed.
3. Relaunch the same binary and resume the same session without invoking a layout command.
4. Verify startup automatically returns the attached workspaces to a horizontal row.
5. Inspect `runtime.yaml` and capture a second screenshot after relaunch.

## Detached workspace and startup paths

1. Detach one workspace into its own native window. Record both root and detached window geometry
   using exact-PID window membership rather than title alone.
2. Relaunch into that persisted state. Verify the detached workspace canvas position and native
   window geometry are unchanged while the attached workspaces form a row.
3. Trigger `Cmd+Shift+A`; verify the detached workspace remains excluded.
4. Exercise the startup session chooser concretely: keep the first exact-PID process and its live
   lease running, then launch a second exact-head process with the same isolated environment and no
   `--new-session`. Verify the `LiveConflict` chooser, choose `Open Copy`, and verify the copied
   session is organized only after activation. Track and clean up both exact PIDs.
5. If an asynchronous agent-session restore is available, verify the loading view completes before
   the restored board is organized, without a loading flash or later snap.
6. Launch with zero attached workspaces and with one attached workspace. Verify both are stable no-ops
   with no repeated layout or repaint loop.

## Resize and visual regression

1. Resize the root window wider and narrower after startup.
2. Verify organization does not rerun or oscillate, panel content remains clipped correctly, and the
   minimap matches canvas geometry.
3. Capture a final screenshot after resize and inspect it for missing panels, clipped labels, stale
   workspace backgrounds, overlap, or Metal rendering artifacts.

## Report and cleanup

Report the exact tested SHA and concise pass/fail evidence for every section, including exact PID and
window inventory, before/after runtime positions, screenshots, and any video or trace. Fix and push
issues that are in scope, then repeat the affected lanes on the new head. End a completed report with
this exact standalone line:

```text
SMOKE-TEST: DONE
```

The implementation agent removes this temporary plan after the completed report and performs the
required final-head verification before declaring the pull request ready.

Use this report shape in the pull-request comment:

```text
SMOKE-TEST REPORT (macOS)
- Exact head and environment: pass | fail — <SHA, macOS, arch, Rust, display>
- Build and exact-PID launch: pass | fail — <PID and window inventory>
- Default-disabled compatibility: pass | fail — <missing-field positions and Settings state>
- Cold-start organization: pass | fail — <runtime positions and screenshot>
- One-shot and Cmd+Shift+A parity: pass | fail — <evidence>
- Settings next-session-load behavior: pass | fail — <false/true config and loaded positions>
- Relaunch persistence: pass | fail — <runtime positions and screenshot>
- Detached/startup edge cases: pass | fail — <evidence>
- Resize and Metal/Retina visuals: pass | fail — <screenshot or motion trace>
Summary: <fixes pushed, remaining issues, or clean pass>
SMOKE-TEST: DONE
```
