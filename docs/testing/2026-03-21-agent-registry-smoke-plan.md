# Agent Registry Smoke Test Plan

Date: 2026-03-21
Branch: `feat/agent-registry`

## Goal

Validate the generic built-in agent refactor and confirm that existing agent presets still behave correctly while new Gemini CLI and KiloCode presets appear and launch as expected.

## Environment

- Use the `feat/agent-registry` worktree.
- Start from a clean `~/.horizon/config.yaml` backup or disposable test home.
- If available, install local CLIs for:
  - `codex`
  - `claude`
  - `opencode`
  - `gemini`
  - `kilo`
- If some CLIs are unavailable, still validate preset visibility, persistence, and non-launch regressions for the others.

## Baseline

1. Launch Horizon and open the presets/settings view.
2. Confirm existing built-in presets are still present:
   - `Codex`
   - `Codex (YOLO)`
   - `Claude Code`
   - `Claude Code (Auto)`
   - `OpenCode`
   - `OpenCode (Fresh)`
3. Confirm new built-in presets are present:
   - `Gemini CLI`
   - `KiloCode`
   - `KiloCode (Fresh)`
4. Confirm panel badges/icons render distinctly for:
   - Codex
   - Claude
   - OpenCode
   - Gemini
   - KiloCode

## Launch And Resume

1. Launch a `Codex` panel and confirm it starts normally.
2. Relaunch or restart the same `Codex` panel and confirm existing Codex session behavior is unchanged.
3. Launch a `Claude Code` panel and confirm it starts normally.
4. Relaunch or restart the same `Claude Code` panel and confirm existing Claude resume behavior is unchanged.
5. Launch an `OpenCode` panel and confirm it starts normally.
6. Relaunch or restart the same `OpenCode` panel and confirm existing OpenCode session behavior is unchanged.
7. Launch a `Gemini CLI` panel and confirm Horizon starts `gemini` without injecting Codex/Claude/OpenCode-specific resume flags.
8. Launch a `KiloCode` panel with default `Last` resume and confirm Horizon starts `kilo --continue`.
9. Launch `KiloCode (Fresh)` and confirm Horizon starts `kilo` without `--continue`.

## Persistence And Bootstrap

1. Save state with open Codex, Claude, and OpenCode panels that already have resumable sessions.
2. Restart Horizon and confirm those three panels still recover through the existing session-binding flow.
3. Save state with open Gemini and KiloCode panels.
4. Restart Horizon and confirm:
   - Gemini does not block startup waiting for a session-catalog bootstrap.
   - KiloCode does not block startup waiting for a session-catalog bootstrap.
   - Existing non-agent panels still restore normally.

## Plugin And Skill Export

1. Launch Horizon once and inspect exported integration files.
2. Confirm Claude assets are still synced under `~/.horizon/plugins/claude-code`.
3. Confirm Codex skill export still exists under:
   - `~/.horizon/integrations/codex/horizon-notify`
   - `~/.agents/skills/horizon-notify`
4. Confirm KiloCode skill export now exists under:
   - `~/.kilocode/skills/horizon-notify`

## Visual Regression Checks

1. Capture a screenshot after launch with one panel of each built-in agent kind visible.
2. Capture a screenshot of the presets UI showing Gemini CLI and KiloCode entries.
3. Verify panel titlebars, focus state, and icon labels remain legible on desktop and on a narrower window width.

## Expected Result

- Existing Codex, Claude, and OpenCode behavior is unchanged.
- Gemini CLI appears as a first-class built-in agent preset and launches cleanly.
- KiloCode appears as a first-class built-in agent preset and supports `Last` via `--continue`.
- Startup/bootstrap logic does not stall on Gemini or KiloCode waiting for unsupported exact session catalogs.
- Panel chrome and preset UI render all built-in agents correctly.
