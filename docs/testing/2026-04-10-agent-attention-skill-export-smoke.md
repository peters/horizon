# Smoke Plan: Agent Attention Skill Export

## Purpose

Validate that Horizon exports the bundled `horizon-notify` integration to the current Codex skill home and that agent attention notifications still surface correctly in the Horizon UI for both Codex and Claude.

## Machine Requirements

- Linux or macOS desktop session with a visible Horizon window
- Local Horizon build prerequisites already installed
- `codex` CLI installed
- `claude` CLI installed if Claude validation is in scope on the machine
- Access to the user home directory used by the Horizon process

## Build + Launch

1. Open a terminal in the Horizon checkout.
2. Run `cargo build`.
3. Launch `target/debug/horizon`.
4. Wait for Horizon to finish startup before opening agent panels.

## Preflight Checks

1. Open Horizon settings and confirm `Features -> Attention Feed` is enabled.
2. Confirm the attention feed overlay is visible when attention items exist.
3. Quit Horizon.
4. Remove any stale exported Codex skill copy if you need a cold-start install check:
   `rm -rf ~/.codex/skills/horizon-notify`
5. Relaunch `target/debug/horizon`.
6. Confirm `~/.codex/skills/horizon-notify/SKILL.md` now exists.
7. Confirm the legacy export still exists at `~/.agents/skills/horizon-notify/SKILL.md`.
8. Confirm the embedded Horizon integration copy still exists at `~/.horizon/integrations/codex/horizon-notify/SKILL.md`.

## Baseline Checks

1. Open one Codex panel and one Claude panel.
2. Confirm both agent panels launch with ordinary terminal output and no immediate false-positive attention item.
3. Confirm existing panel focus, sidebar selection, and workspace navigation behave normally before any notification is triggered.

## Direct Notification Path

1. In the Codex panel, run:
   `printf '\033]0;HORIZON_NOTIFY:attention:Codex smoke test\007' > "/dev/$(ps -o tty= -p $PPID | tr -d ' ')"`
2. Confirm Horizon shows a new high-severity attention item in the feed.
3. Confirm the Codex panel titlebar shows an attention badge.
4. Confirm the containing workspace/sidebar row shows attention state.
5. Dismiss the item and confirm it clears from the open list.
6. Repeat the same command in the Claude panel with `Claude smoke test`.
7. Confirm the same UI behavior occurs for Claude.

## Agent-Driven Attention Checks

1. In the Codex panel, ask Codex to stop and ask for a user decision before continuing.
2. Confirm an attention item appears when Codex reaches the point where it needs input.
3. Click `Go to panel` from the feed and confirm Horizon focuses the correct Codex panel.
4. In the Claude panel, ask Claude to stop and ask for a user decision before continuing.
5. Confirm an attention item appears when Claude reaches the point where it needs input.
6. Confirm the resulting item is tied to the Claude panel, not the Codex panel.

## Edge Cases

1. Trigger two attention items back-to-back from the same Codex panel and confirm the newest item appears first in the feed.
2. Trigger one Codex and one Claude attention item close together and confirm both remain independently navigable.
3. Trigger an `info` severity item and confirm it renders with lower emphasis than `attention`.
4. Trigger a `done` severity item and confirm it renders as medium severity.
5. Dismiss an item, then trigger the same notification again and confirm it can reappear after the signal clears and reoccurs.

## Persistence + Restart Checks

1. With Horizon closed, remove `~/.codex/skills/horizon-notify` again.
2. Relaunch Horizon and confirm the skill is recreated without manual copying.
3. Trigger one Codex notification after relaunch and confirm it still appears.
4. Trigger one Claude notification after relaunch and confirm it still appears.

## Visual Regression Checks

1. Confirm the attention feed stays readable and anchored correctly with the default window size.
2. Resize the Horizon window narrower and taller, then confirm the feed remains usable and does not overlap core controls in a broken way.
3. Confirm attention badges in panel chrome remain legible and do not clip the title text beyond the expected truncation behavior.
4. Confirm sidebar attention rows remain readable when both Codex and Claude have open items.

## Failure Evidence To Save

- Which agent failed: Codex, Claude, or both
- Whether the failure was export/install, direct OSC notification, or agent-driven notification
- Exact contents of `~/.codex/skills/horizon-notify`
- Screenshot of the Horizon UI after the failed notification
- Any terminal output showing the agent reached a user-input prompt without creating attention

## Explicit Non-Goals For This Pass

- No desktop notification or OS-level window urgency validation
- No release-build benchmarking
- No Windows installer validation
- No unrelated session-restore or detached-workspace regression sweep
