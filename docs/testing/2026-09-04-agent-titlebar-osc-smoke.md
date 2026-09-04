# Smoke: agent-set panel titles

Temporary plan for `fix/sticky-agent-terminal-title`. Delete after the UI pass.

## Setup

1. Launch the candidate from this worktree with an isolated config (`--config` + `--ephemeral`).
2. After launch, confirm Horizon wrote `horizon-notify/SKILL.md` into every built-in agent home (use `%USERPROFILE%` on Windows if `HOME` is unset):
   - `~/.agents/skills/horizon-notify/SKILL.md`
   - `~/.claude/skills/horizon-notify/SKILL.md`
   - `$GROK_HOME/skills/horizon-notify/SKILL.md` when `GROK_HOME` is set, otherwise `~/.grok/skills/horizon-notify/SKILL.md`
   - `$CODEX_HOME/skills/horizon-notify/SKILL.md` when `CODEX_HOME` is set, otherwise `~/.codex/skills/horizon-notify/SKILL.md`
   - `~/.config/opencode/skills/horizon-notify/SKILL.md`
   - `~/.gemini/skills/horizon-notify/SKILL.md`
   - `~/.kilocode/skills/horizon-notify/SKILL.md`
   - `~/.pi/agent/skills/horizon-notify/SKILL.md`
3. Pass: each file exists and contains `HORIZON_TITLE:set`.

## Lanes

### 1. Ordinary TUI titles still appear

Open any built-in agent panel. Codex is enough for this lane.

- Before the agent sets a Horizon title, the panel titlebar may show the TUI OSC 0 title (often project name, sometimes a spinner prefix).
- Pass: the titlebar is not stuck on the preset name alone if the TUI is emitting OSC 0.

### 2. Agent `HORIZON_TITLE:set` wins

In the agent panel (or a nested tool), run:

```sh
printf '\033]0;HORIZON_TITLE:set:PR comments: Docker lifecycle fixes\007' > "/dev/$(ps -o tty= -p $$ | tr -d ' ')"
```

- Pass: titlebar becomes `Codex — PR comments: Docker lifecycle fixes` (or the custom panel name plus that suffix).
- Pass: it stays that way while the agent TUI keeps redrawing (cwd/spinner OSC 0 must not revert it to `: horizon` or similar).

### 3. Clear restores ordinary titles

```sh
printf '\033]0;HORIZON_TITLE:clear\007' > "/dev/$(ps -o tty= -p $$ | tr -d ' ')"
```

- Pass: the pinned title disappears.
- Pass: a later ordinary OSC 0 title from the TUI can show again.

### 4. Each built-in agent can pin the titlebar

For each preset **Codex**, **Claude**, **Grok**, **OpenCode**, **Gemini**, **Pi**, and **Kilo**:

1. Open a panel from that preset.
2. Confirm `HORIZON=1`.
3. Run the `HORIZON_TITLE:set` command from lane 2.

- Pass: the titlebar shows that panel's name plus `PR comments: Docker lifecycle fixes`.
- Pass: later TUI OSC 0 cwd/spinner titles do not revert it.

### 5. Skill path

If the `gh-address-comments` (or equivalent) `set_terminal_title.py` skill is available, have the agent set a title for the current task.

- Pass: the panel titlebar shows that title within a second and keeps it while the agent is working.

## Out of scope

- OS window title
- Persisting the pinned title across Horizon restarts
