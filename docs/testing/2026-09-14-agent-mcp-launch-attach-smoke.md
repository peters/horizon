# Smoke: launch-time Pi / Grok / OpenCode / Antigravity MCP attach

Temporary plan for the process-scoped Horizon browser MCP attachments added after #626.

Use an isolated `HOME` and `--ephemeral`. Unset `HORIZON` for the child. Launch `target/debug/horizon`.

## Lanes

1. **Attach on start** — After Horizon maps, confirm these files contain a Horizon-managed `horizon-browser` stdio server pointing at this process:
   - `$HOME/.pi/agent/mcp.json` (`env.HORIZON_BROWSER_MCP_LEASE = 1`)
   - `$HOME/.gemini/config/mcp_config.json`
   - `$HOME/.grok/config.toml` (`[mcp_servers.horizon-browser]` inside the Horizon-managed markers)
   OpenCode must **not** gain a new `~/.config/opencode/opencode.json`. Launch an OpenCode panel and confirm the process environment has `OPENCODE_CONFIG_CONTENT` with `horizon-browser`.
2. **Browser skill** — Confirm `horizon-browser/SKILL.md` exists under Pi, OpenCode, Antigravity CLI, and Grok skill homes, and does **not** exist under `~/.kilocode/skills` or `~/.agents/skills`.
3. **Preserve user servers** — Seed Pi `mcp.json` with an unrelated `github` server before launch; after attach it must still be present beside `horizon-browser`.
4. **Last host** — Close the Horizon window through the window manager. After the process exits, managed `horizon-browser` must be gone from Pi / Antigravity / Grok configs. The seeded `github` server must remain. A user-owned `horizon-browser` without the lease marker must be left untouched.
5. **Two hosts** — Start a second isolated Horizon against the same `HOME` before closing the first. Closing the first must leave `horizon-browser` in place; closing the last must remove it.

## Not in this plan

Claude `--plugin-dir` / `.mcp.json` and Codex `-c mcp_servers.horizon-browser.*` are unchanged.
