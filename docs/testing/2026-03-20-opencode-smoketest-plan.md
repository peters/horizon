# OpenCode Support Smoke Test Plan

This is a temporary validation artifact for the `feat/opencode-support` branch.
Use it to verify the exact branch or commit that will be pushed for review.

## Scope

Validate that Horizon supports OpenCode as a first-class agent panel on macOS:

- OpenCode presets exist after config migration.
- Fresh and resumed OpenCode panels launch correctly.
- `Last` resume binds to the expected root session for the current working directory.
- Explicit session resume keeps the exact bound session across restart.
- OpenCode usage appears in the dashboard without regressing Claude/Codex cards.
- Failure cases degrade safely when OpenCode or its local DB is unavailable.

## Environment

Record the following before testing:

- macOS version and architecture.
- Terminal app used to launch Horizon: Terminal, iTerm2, Kitty, Warp, or other.
- Horizon commit SHA under test.
- OpenCode version from `opencode --version`.
- Absolute path to the Horizon checkout under test.
- Absolute path to the disposable test repo.
- Absolute path to `~/.horizon/config.yaml`.
- Absolute path to `~/.local/share/opencode/opencode.db` if present.

## Preconditions

1. Confirm no stale Horizon processes are running:

   ```bash
   pgrep -fl horizon
   ```

2. Confirm OpenCode is installed and on `PATH`:

   ```bash
   which opencode
   opencode --version
   ```

3. Back up the active Horizon config:

   ```bash
   cp ~/.horizon/config.yaml ~/.horizon/config.yaml.pre-opencode-smoke
   ```

4. Build the exact branch under test:

   ```bash
   cargo build
   ```

5. Launch Horizon from the branch checkout, not from another installed binary.

## Disposable Fixture Setup

1. Create a disposable repo:

   ```bash
   mkdir -p ~/tmp/horizon-opencode-smoke
   cd ~/tmp/horizon-opencode-smoke
   git init
   echo "fixture" > README.md
   git add README.md
   git commit -m "fixture"
   ```

2. Seed OpenCode sessions in the disposable repo. The goal is to end with:

- one older root session in the repo directory,
- one newer root session in the repo directory,
- one root session in a different directory,
- one child or fork session if practical,
- one archived session if practical.

3. If the CLI supports it in your installed version, capture the sessions as JSON:

   ```bash
   opencode session list --format json > /tmp/opencode-sessions-before.json
   ```

4. Record the expected latest root session ID for the disposable repo. This is the session Horizon should bind for `resume: last`.

## Config Migration

1. Replace `~/.horizon/config.yaml` with a v2-style config that does not contain OpenCode presets.
2. Launch Horizon once.
3. Verify `~/.horizon/config.yaml` now contains:

- `version: 3`
- `name: OpenCode`
- `name: OpenCode (Fresh)`
- `kind: open_code`

4. Quit Horizon.
5. Launch Horizon again.
6. Verify the config file does not gain duplicate OpenCode presets on the second launch.
7. If you injected custom presets with overlapping names or aliases, verify they were preserved and not rewritten.

Expected result:

- migration is additive and idempotent,
- custom presets remain intact,
- the OpenCode defaults appear once.

## Preset Discovery

1. Open the command palette or preset picker.
2. Search for `open`.
3. Search for `oc`.
4. Search for `ocf`.
5. Confirm these entries are visible:

- `OpenCode`
- `OpenCode (Fresh)`

6. Capture a screenshot of the search results.

Expected result:

- the labels are correct,
- the aliases resolve correctly,
- the OpenCode entries are visually distinct from Claude/Codex presets.

## Fresh OpenCode Launch

1. Open the disposable repo in Horizon.
2. Launch the `OpenCode (Fresh)` preset.
3. Verify the panel opens and the OpenCode TUI is interactive.
4. Type a short prompt such as `reply with the word ready only` and submit it.
5. Verify keyboard input, cursor motion, and prompt submission work normally.
6. Resize the panel smaller and larger.
7. Verify the TUI redraw remains stable and does not visibly corrupt.
8. Capture a screenshot of the fresh OpenCode panel.

Expected result:

- no crash,
- no unexpected resume into an old session,
- panel chrome identifies the panel as OpenCode,
- the panel remains interactive after resize.

## Last Resume Selection

1. In the disposable repo, ensure two root OpenCode sessions exist and the newer one is known.
2. Launch the `OpenCode` preset.
3. Verify Horizon resumes the newest root session for the current repo.
4. Verify it does not resume:

- a child or fork session,
- an archived session,
- a session from another directory.

5. Capture a screenshot after the resumed session is visible.

Expected result:

- `resume: last` chooses the most recent resumable root session scoped to the repo directory.

## Explicit Session Resume

1. Create or edit a preset or runtime state entry that uses `resume: session` with a known OpenCode session ID.
2. Launch that panel.
3. Confirm the visible session matches the exact configured ID.
4. Quit Horizon cleanly.
5. Relaunch Horizon.
6. Confirm the same panel still resumes the same exact session.

Expected result:

- explicit session binding is stable across restart,
- Horizon does not silently replace it with the most recent session.

## Persistence Across Restart

1. Arrange a workspace containing:

- one `OpenCode (Fresh)` panel,
- one `OpenCode` resumed panel,
- one non-OpenCode control panel such as Shell or Claude.

2. Quit Horizon cleanly.
3. Relaunch Horizon.
4. Verify:

- workspace placement persists,
- panel placement persists,
- OpenCode panels restore as OpenCode panels,
- resumed OpenCode panels do not duplicate,
- the control panel still behaves normally.

5. Capture a screenshot after restore.

Expected result:

- persistence keeps panel kind, position, and session binding intact.

## Usage Dashboard

1. Open the Usage dashboard after the seeded OpenCode sessions have assistant activity.
2. Verify a distinct OpenCode card appears alongside Claude and Codex.
3. Verify today and week values are non-zero when the fixture supports it.
4. Compare one or two headline values against local OpenCode data:

   ```bash
   opencode stats --days 7
   ```

5. Capture a screenshot of the dashboard.

Expected result:

- OpenCode appears as a separate tool,
- dashboard layout remains readable,
- Claude and Codex values still render,
- numbers are directionally consistent with OpenCode’s local data.

## Visual Checks

Inspect the following manually:

- OpenCode panel chrome icon and accent are readable in focused and unfocused states.
- Long titles do not overlap close buttons or panel badges.
- Workspace switching does not break OpenCode panels.
- Command palette selection still works after opening and closing OpenCode panels.
- No obvious redraw flicker occurs while interacting with the OpenCode TUI.

Capture screenshots if any visual issue appears.

## Failure Cases

### OpenCode Missing From PATH

1. Temporarily launch Horizon with `PATH` modified so `opencode` is unavailable.
2. Try to open an OpenCode preset.

Expected result:

- Horizon reports launch failure cleanly,
- Horizon does not crash,
- other panel kinds still work.

### OpenCode DB Missing

1. Temporarily move the DB aside:

   ```bash
   mv ~/.local/share/opencode/opencode.db ~/.local/share/opencode/opencode.db.smoke-backup
   ```

2. Launch Horizon.
3. Open `OpenCode (Fresh)`.
4. Open the Usage dashboard.
5. Restore the DB afterward:

   ```bash
   mv ~/.local/share/opencode/opencode.db.smoke-backup ~/.local/share/opencode/opencode.db
   ```

Expected result:

- fresh OpenCode launches still work,
- the Usage dashboard degrades gracefully,
- `resume: last` does not crash Horizon when the DB is absent.

### Empty OpenCode Session Store

1. Test with an empty or near-empty DB if practical.
2. Launch the `OpenCode` preset in a repo with no matching stored sessions.

Expected result:

- Horizon falls back to a fresh OpenCode launch,
- no false resume occurs.

## Evidence To Collect

Save the following artifacts:

- Horizon commit SHA.
- `opencode --version` output.
- screenshots for preset discovery, fresh launch, resumed launch, restored workspace, and usage dashboard.
- terminal transcript of validation commands.
- relevant Horizon logs if a failure occurs.

If you uncover a motion-sensitive visual bug, capture short video instead of screenshots alone.

## Cleanup

1. Restore the original config:

   ```bash
   mv ~/.horizon/config.yaml.pre-opencode-smoke ~/.horizon/config.yaml
   ```

2. Remove only disposable test data created for this run.
3. Delete this file after the smoke pass unless the branch owner explicitly wants it kept.
