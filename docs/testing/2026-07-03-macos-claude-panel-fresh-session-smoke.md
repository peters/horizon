# macOS Claude Panel Fresh Session Smoke Plan

## Scope

Validate that adding a Claude Code panel to a workspace **always** starts a
new Claude session, never resuming an old conversation, on macOS with Claude
Code v2.1.199 or newer. This exercises the add-panel normalization in
`crates/horizon-ui/src/app/actions/panels.rs`, the removal of launch-time
session scavenging in `crates/horizon-ui/src/app/session.rs`, and the new
live-session registry guard in
`crates/horizon-core/src/runtime_state/claude_live_sessions.rs`.

Background: presets with `resume: last` used to reattach a newly added panel
to the most recent catalog session for that cwd, with no check for whether
another process already had the session open. Before Claude Code v2.1.199
this was masked by empty leftover session files; from v2.1.199 the CLI no
longer creates or refreshes those, so new panels visibly resumed real old
conversations. New panels now always launch with a fresh `--session-id`, and
that id is stored as the panel's session binding from the start, so a restart
resumes exactly each panel's own conversation (`--resume` when the transcript
exists, a same-id fresh launch when the panel was never used). Reconnecting
to an old session is explicit only: the panel context menu rebind, or restart
restore of a panel's own binding.

Note on liveness: the pid check for stale registry entries is Linux-only
(`/proc`). On macOS every `~/.claude/sessions/` entry is treated as live,
which only ever biases toward starting a fresh session; clean exits remove
registry entries, so the common paths behave identically.

## Prerequisites

- macOS 14 or newer on Apple Silicon or Intel.
- Claude Code v2.1.199 or newer, authenticated (`claude --version`).
- Build Horizon from the PR branch:
  ```bash
  git switch fix/claude-panel-fresh-session
  cargo build
  ```

## Test Project Setup

Use a scratch project directory with a pre-existing recent Claude session:

```bash
export SMOKE_PROJ="$(mktemp -d "$HOME/horizon-claude-smoke.XXXXXX")"
cd "$SMOKE_PROJ"
claude --model claude-haiku-4-5 -p "Reply with exactly: seeded" >/dev/null
ls ~/.claude/projects/ | grep horizon-claude-smoke
```

The print-mode call leaves one completed session on disk for this cwd. Do not
wait; the point is that even a seconds-old session must not be picked up.

## Steps

1. **Fresh session on add (the reported bug).** Launch the built Horizon,
   create a workspace with cwd `$SMOKE_PROJ`, and add a panel from the
   `Claude Code` preset (`resume: last`). Expected: the panel shows a
   brand-new empty Claude conversation, not the seeded one, even though the
   seeded session is recent. The launch log line (`launching agent panel`)
   must show `should_resume=false` and a `--session-id` argument, not
   `--resume`.

2. **Every add is fresh.** Send a short prompt in the first panel, then add a
   second and a third Claude Code panel to the same workspace. Expected: each
   panel is a fresh empty conversation; none shows another panel's
   conversation.

3. **Explicit rebind still works.** Open a panel's context menu and rebind it
   to the seeded session. Expected: the panel relaunches with `--resume` and
   shows the seeded conversation. Explicit selection remains the only way to
   attach an old session to a new panel.

4. **Restart resumes each panel's own session.** Send distinct prompts in two
   fresh Claude panels in the same workspace (for example "remember apple" and
   "remember banana"), quit Horizon normally, and relaunch it. Expected: each
   panel reconnects to its own conversation via `--resume` with no
   cross-swapping between the two panels.

5. **Never-used panels restart fresh without errors.** Add a Claude Code
   panel, type nothing, quit Horizon, and relaunch. Expected: the panel comes
   back as a working fresh Claude prompt; it must not show a
   "No conversation found with session ID" error. The relaunch log shows
   `--session-id` with the panel's original id.

## Cleanup

```bash
rm -rf "$SMOKE_PROJ"
```

Optionally remove the seeded/test sessions under
`~/.claude/projects/<munged-$SMOKE_PROJ>/`.
