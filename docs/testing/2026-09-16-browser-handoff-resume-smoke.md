# Browser handoff resume — smoke test plan (temporary)

Validates that **any MCP client** waiting on `browser_handoff` resumes when the
user selects **Done — hand back to agent**: Codex, Claude, Grok, and
`horizon-browser` CLI (`run` plans and quoted-goal jobs). Before this change
the tool returned immediately with `handoff_pending: true` and told the client
to poll `browser_list`. Agents ended the turn, so the hand-back button did
nothing visible.

Build: `cargo build` (debug `target/debug/horizon` is enough). Isolated
runtime only: temp `HOME`, `--config`, `--ephemeral`. Unset `HORIZON` on the
child. Identify windows by the candidate PID. Do not reuse the operator's live
Horizon process.

## Shared preconditions

- Linux: task-owned Xvfb + openbox; pick an unused display.
- `codex` and `claude` on PATH for lanes C and D. `grok` on PATH for lane F
  (the CLI prefers Grok when present).
- Scope screenshots, `xdotool`, and process checks to the candidate PID.
- After the run: close the exact window normally, confirm the candidate
  exited, remove the temp config and proof artifacts.

## Lane A — MCP contract (no agent TUI)

These prove the resume path the TUIs actually call.

### A1. Wait-until-hand-back (automated)

Already covered by `cargo test -p horizon-browser-mcp --test workspace_scope`:

- `browser_handoff` with default `wait` blocks until `handoff.done` is set,
  then returns `handoff_pending: false`.
- `timeout_millis: 1000` returns a typed timeout and **leaves the request
  pending** so a later hand-back still works.
- `wait: false` still returns immediately (workspace-scope and CLI plan tests).

Re-run after any fix:

```bash
cargo test -p horizon-browser-mcp --test workspace_scope -- --nocapture
cargo test -p horizon-core panel::spawn::tests::default_codex_launch -- --nocapture
cargo test -p horizon-ui plugin_install -- --nocapture
cargo test -p horizon-browser-cli successive_process_local_runs run_waits_for_hand_back grok_home_contains_only -- --nocapture
```

Expected: Codex launch args include `tool_timeout_sec=3660`. Claude plugin
`.mcp.json` includes `"timeout": 3660000`. Grok CLI job `config.toml` includes
`tool_timeout_sec = 3660`. A CLI `run` of `browser_handoff` (default wait)
blocks until `done` is set, then reports `handoff_pending: false`. Plans that
must not block pass `"wait": false`.

### A2. Live Chromium MCP gate (interactive hand-back)

Isolated Horizon, visible browser, **blocking** `browser_handoff` on a second
MCP stdio client (same actor as a Codex/Claude panel). While it is in flight:

1. `browser_panel` reports `handoff_pending: true`.
2. `browser_act reload` fails with "would block".
3. Banner **Done — hand back to agent** is visible in the candidate window.
4. Click that exact button.
5. The blocking call returns `handoff_pending: false` and a non-zero
   `elapsed_millis`.
6. A following `browser_snapshot` succeeds (the "agent resumed" signal).
7. Audit has exactly one `handoff_requested`, one rejected reload, one
   user `handoff_done`, in that order.

```bash
cargo build
python3 scripts/browser-smoke/run.py \
  --backend chromium \
  --horizon target/debug/horizon \
  --ephemeral
```

Do **not** pass `--skip-handoff`. The runner now starts the blocking call
before prompting; click **Done — hand back to agent** in the printed PID's
window. Repeat on Firefox when shared handoff code changed.

### A3. Heartbeat while waiting

During A2, before clicking Done, wait at least 12 seconds (owner TTL is 10 s).
`browser_panel` must still show `owned_by_caller: true`. Then click Done.
The blocking call must still return success. This is the regression where a
silent wait would drop the lease and never resume.

### A4. User-active 5 s vs explicit handoff

Click/type in the page without requesting handoff: agent actions must resume
after five seconds. Then request handoff: actions stay blocked until Done,
not merely five seconds.

## Lane B — launch wiring (Codex and Claude)

No live model required.

### B1. Codex panel command

Create a default Codex panel in an isolated session. From
`launching agent panel` traces, the child command must contain:

- `mcp_servers.horizon-browser.command=`
- `mcp_servers.horizon-browser.args=["--browser-mcp"]`
- `mcp_servers.horizon-browser.env_vars=["HORIZON_BROWSER_ACTOR","HORIZON_BROWSER_HOST_INSTANCE"]`
- `mcp_servers.horizon-browser.default_tools_approval_mode="approve"`
- `mcp_servers.horizon-browser.tool_timeout_sec=3660`

A 60-second tool timeout here is a fail: Codex would kill `browser_handoff`
before the user can hand back.

Custom Codex commands must still **not** receive this registration.

### B2. Claude plugin MCP config

Isolated boot must write the host plugin dir `.mcp.json` with
`--browser-mcp` and `"timeout": 3660000`. The bundled `horizon-browser`
skill must tell the model that `browser_handoff` **waits** and not to poll
`browser_list` for hand-back. Confirm the skill on disk matches
`assets/plugins/claude-code/skills/horizon-browser/SKILL.md` (and the Codex
copy).

`HORIZON_BROWSER_ACTOR` and `HORIZON_BROWSER_HOST_INSTANCE` must still be
injected into the Claude panel environment and forwarded to the MCP child.

## Lane C — live Codex panel

Requires `codex` on PATH and a working login.

1. Task-owned Xvfb + openbox. Isolated `HOME` and `--config`. Unset
   `HORIZON` on the child. `--ephemeral`. Record the exact PID.
2. Create a **Codex** panel in the same workspace as the upcoming browser
   (default workspace is fine). Confirm B1 launch args from traces.
3. In the Codex panel, send: create a browser in this workspace, navigate to
   a simple page, then `browser_handoff` with reason `smoke sign-in`.
4. Codex must **not** return to the idle prompt while the banner is up. The
   TUI should stay on the in-flight `browser_handoff` tool (Working).
5. Click **Done — hand back to agent** in the browser panel.
6. Within a few seconds Codex must leave Working, receive
   `handoff_pending: false`, take a fresh snapshot (or say it will), and
   continue the turn **without** a new user message.
7. A follow-up `browser_act` or snapshot must succeed. If Codex instead sits
   idle at the prompt after Done, this lane fails.
8. Close the exact window normally. Confirm the candidate PID exited.

Proof: screenshot of the waiting banner, screenshot or trace after Done
showing Codex continued, and the MCP/audit records for that panel.

Optional long-wait: leave the banner up for 90 seconds (past the old 60 s
Codex tool timeout) then click Done. Resume must still work.

## Lane D — live Claude panel

Requires `claude` on PATH and a working login. Same isolation as lane C.

1. Create a **Claude** panel. Confirm B2 plugin dir, skill text, actor, and
   host instance forwarding with a real in-panel `browser_create`.
2. Ask Claude to open a browser and `browser_handoff` for a consent dialog.
3. Claude must keep the tool in flight (Working), not end the turn.
4. Click **Done — hand back to agent**.
5. Claude must continue the same turn from a fresh snapshot, without the
   operator typing into the Claude panel.
6. 90-second hold before Done: still resumes (idle-abort / timeout floor).
7. Close normally; candidate PID gone.

## Lane E — edges

| Case | Expected |
| --- | --- |
| `wait: false` from CLI/script | Returns immediately; `handoff_pending: true`; later Done still clears it |
| Hand-back with no live driver | UI shows retry error; MCP wait stays pending |
| Panel moved out of the agent's workspace during wait | Blocking call errors with the workspace rejection; destination agents are not stuck |
| Second `browser_handoff` while one is pending | Replaces the request; UI shows the new reason; waiters see the new pending request |
| Hidden panel handoff | Banner still appears when shown; Done still completes a blocking wait |
| Process-local CLI `run` of a wait:false handoff plan | Completes and releases ownership immediately (existing CLI test) |
| CLI `run` of default `browser_handoff` | Blocks until Done, then `handoff_pending: false` |

## Lane F — Grok CLI (quoted-goal job)

Requires `grok` on PATH (the CLI prefers it over Codex) and a working login.

Horizon Grok **TUI panels** do not receive browser MCP injection; the Grok
path this product already ships is `horizon-browser "<goal>"`, which writes
an isolated `GROK_HOME` with `horizon-browser` MCP and
`tool_timeout_sec = 3660`.

1. Isolated temp `HOME`. Launch a live Horizon with a visible browser panel
   (same isolation as A2) **or** reuse the MCP gate window.
2. From a second process, with `HORIZON_BROWSER_ACTOR` matching a workspace
   member (or the process-local fallback against an unstamped test
   manifest), run:

   ```bash
   HORIZON_BROWSER_AGENT_COMMAND=grok target/debug/horizon-browser \
     "Call browser_list, then browser_handoff on the first panel with reason grok smoke sign-in, then take a snapshot after it returns."
   ```

   If no live panel exists, the goal may `browser_create` first.
3. While the job is in flight, the browser banner must show waiting.
   Grok must not finish the job before Done.
4. Click **Done — hand back to agent**.
5. The job must continue, snapshot, and exit 0 with `ok: true` in the
   report. `tool_timeout_sec = 60` here is a fail (old Codex/Grok client
   timeout).
6. Optional 90-second hold before Done: still resumes.

Proof: job `GROK_HOME/config.toml` contains `tool_timeout_sec = 3660`,
the blocking wait, Done, and a successful report.

## Pass / fail

Pass only if A1–A4, B1–B2, the CLI hand-back test, and at least one of C, D,
or F on this machine are green on the **exact candidate head**. Remaining
agent/CLI lanes may be handed to a second machine via a `SMOKE-TEST REQUEST`
comment. A blocking `browser_handoff` that returns before Done, or a client
that stays idle after Done, is a fail.
