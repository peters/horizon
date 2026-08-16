# Grok Build agent panel — smoke test plan (temporary)

Validates the new `Grok` panel kind (xAI Grok Build CLI, binary `grok`).
Each lane is self-contained: no Horizon-specific context is needed beyond
this file. The `grok` CLI must be installed on PATH for the TUI lanes
(`npm install -g @xai-official/grok` is the supported install; it is not
required for the catalog/migration lanes).

## Shared preconditions

- Build: `cargo build` (debug binary `target/debug/horizon` is sufficient).
- Isolated runtime state: use a temp `HOME` so the tester's real
  `~/.horizon` and `~/.grok` state is untouched:
  ```bash
  SMOKE_HOME=$(mktemp -d)/home && mkdir -p "$SMOKE_HOME/.horizon"
  ```
- When multiple Horizon instances may be running, scope every window
  inspection and kill to the exact PID under test (match via
  `tr '\0' '\n' < /proc/<pid>/environ | grep '^HOME='`), never by app name.

## Lane A — Linux / macOS (local, any machine with a GPU or llvmpipe)

### A1. Config migration v8 -> v9

Seed a v8 config (no Grok preset) into the smoke home:

```bash
cp <real ~/.horizon/config.yaml> "$SMOKE_HOME/.horizon/config.yaml"
# then: set `version: 9` -> `version: 8` and delete any `Grok` preset block
```

Launch:

```bash
HOME="$SMOKE_HOME" RUST_LOG=info target/debug/horizon
```

Expected:
- Window maps (`xwininfo -root -tree` on X11; `screencapture`/window list on
  macOS).
- `$SMOKE_HOME/.horizon/config.yaml` is rewritten to `version: 9` with a
  `Grok` preset appended after the last existing preset
  (`alias: gb`, `kind: grok`, `resume: fresh`).
- Log contains `migrated config to version 9`.

### A2. Panel launch (fresh and resume)

Seed a runtime session with two Grok panels so restore drives the launches
(copy the shape below into
`$SMOKE_HOME/.horizon/sessions/<sid>/runtime.yaml`; also create
`meta.yaml` — `profile_id` must equal the FNV-1a hash of the canonical
config path, which Horizon itself writes into other session dirs, so reuse
the profile_id from an existing session of the same smoke home — and list
the session in `sessions/index.yaml` with `last_session_id: <sid>`):

```yaml
workspaces:
- local_id: ws-smoke
  name: Grok Smoke
  cwd: <any existing dir>
  position: [0.0, 0.0]
  template: null
  layout: Grid
  panels:
  - local_id: grok-fresh
    name: Grok Fresh
    kind: grok
    command: null
    args: []
    cwd: <any existing dir>
    rows: 30
    cols: 100
    resume: fresh
    position: [0.0, 0.0]
    size: [780.0, 500.0]
    session_binding: null
    template: null
  - local_id: grok-resume
    name: Grok Resume
    kind: grok
    command: null
    args: []
    cwd: <any existing dir>
    rows: 30
    cols: 100
    resume: !session
      session_id: smoke-session-123
    position: [800.0, 0.0]
    size: [780.0, 500.0]
    session_binding: null
    template: null
```

Launch with the smoke home. Expected (from the `launching agent panel`
trace lines):

- Panel 1: `cmd=... -ic grok` (fresh, no resume flags).
- Panel 2: `cmd=... -ic grok --resume smoke-session-123`.
- Both child processes stay alive (grok TUI shows its auth/welcome screen;
  auth is not required for this smoke).
- Panel title-bar badge reads `GB`.
- Resize the panels and run fit-workspace: no crash, TUI reflows.

### A3. Session catalog (only when `grok` has real sessions)

If the machine has an authenticated Grok account with at least one session:

- Create/finish one session in `<dir>` with the real `grok` CLI.
- Restart Horizon (same or real home) with a Grok panel in `<dir>`.
- The panel's session picker lists the session with a sensible title and
  cwd; picking a session rebinds the panel and relaunches with
  `--resume <session-id>`.
- Without `grok` installed or without a store: picker is empty, no crash,
  no error spam (catalog load is best-effort).

## Lane B — macOS (Apple Silicon)

Same as Lane A, plus:

- Binary install: `npm install -g @xai-official/grok` (pulls the
  darwin-arm64 platform binary).
- Window inspection via window-server tooling instead of xwininfo;
  screenshots via `screencapture -o <id>` or `-V` video for the resize pass.

## Lane C — Windows

Same as Lane A, plus:

- Binary install: `npm install -g @xai-official/grok` (win32-x64 binary).
- `grok` must be resolvable from the PTY login shell (`bash -ic grok`);
  npm global bin must be on PATH.
- No `~/.grok` store: panel launch still works (fresh + resume cmds as in
  A2); session catalog stays empty.

## Pass criteria

- A1/A2/B/C all pass; migration output is byte-identical in shape to the
  documented preset.
- Kill the horizon PID; no orphaned `grok` children survive.

## Cleanup

- Remove the smoke home and any screenshots.
- Delete this plan file once every requested lane reports pass.
