# macOS Claude Panel Fresh Session Smoke Plan

## Scope

Validate that adding a new Claude Code panel starts a **new** Claude session
instead of resuming an old conversation, on macOS with Claude Code v2.1.199 or
newer. This exercises the launch-binding selection fix in
`crates/horizon-ui/src/app/session.rs` (reattach recency window) and the new
live-session registry reader in
`crates/horizon-core/src/runtime_state/claude_live_sessions.rs`.

Background: presets with `resume: last` used to reattach a newly added panel to
the most recent catalog session for that cwd with no recency limit and no check
for whether another process already had the session open. Before Claude Code
v2.1.199 this was masked by empty leftover session files; from v2.1.199 the CLI
no longer refreshes or creates those, so new panels visibly resumed real old
conversations, including sessions live in other panels.

Note on liveness: the pid check for stale registry entries is Linux-only
(`/proc`). On macOS every registry entry is treated as live, which only ever
biases toward starting a fresh session; clean exits remove registry entries, so
the common paths behave identically. Step 4 covers this.

## Prerequisites

- macOS 14 or newer on Apple Silicon or Intel.
- Claude Code v2.1.199 or newer, authenticated (`claude --version`).
- Build Horizon from the PR branch:
  ```bash
  git switch fix/claude-panel-fresh-session
  cargo build
  ```

## Test Project Setup

Use a scratch project directory with pre-existing old Claude sessions:

```bash
export SMOKE_PROJ="$(mktemp -d "$HOME/horizon-claude-smoke.XXXXXX")"
cd "$SMOKE_PROJ"
claude --model claude-haiku-4-5 -p "Reply with exactly: seeded" >/dev/null
```

The print-mode call above leaves one completed session on disk for this cwd
(verify with `ls ~/.claude/projects/ | grep horizon-claude-smoke`). Wait at
least 5 minutes after seeding (or backdate the file mtime with `touch -t`) so
the seeded session falls outside the reattach window.

## Steps

1. **Fresh session on add (the reported bug).** Launch the built Horizon,
   create a workspace with cwd `$SMOKE_PROJ`, and add a panel from the
   `Claude Code` preset (`resume: last`). Expected: the panel shows a brand-new
   empty Claude conversation, not the seeded one. The launch log line
   (`launching agent panel`) must show `should_resume=false` and a
   `--session-id` argument, not `--resume`.

2. **No double-attach to a live session.** In the panel from step 1, send any
   short prompt so its session file exists. Add a second Claude Code panel in
   the same workspace. Expected: the second panel is also a fresh conversation;
   it must not open the first panel's conversation, because that session id is
   listed in `~/.claude/sessions/` while the first panel runs.

3. **Reattach still recovers recent work.** Close the second panel (its claude
   process exits, freeing its registry entry), then within 5 minutes add a new
   Claude Code panel. Expected: the new panel resumes the session the closed
   panel had, restoring its conversation (`--resume <id>` in the launch log).

4. **Restart resume is unaffected.** Quit Horizon normally and relaunch it.
   Expected: the surviving Claude panels reconnect to their own sessions via
   `--resume` and show their previous conversations.

## Cleanup

```bash
rm -rf "$SMOKE_PROJ"
```

Optionally remove the seeded/test sessions under
`~/.claude/projects/<munged-$SMOKE_PROJ>/`.
