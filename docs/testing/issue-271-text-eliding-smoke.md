# Issue 271 macOS text eliding and tooltip smoke plan

## Purpose

Validate the exact issue-271 PR head on macOS with the Metal renderer after
the shared truncation, single-line layout, tooltip, and badge-painting changes.
Every Horizon launch below uses a task-specific temporary home and must be
identified by exact PID. Do not reuse or automate the daily Horizon session.

## Evidence to capture

- Exact PR head SHA, macOS version, CPU architecture, display scale, and Metal
  adapter/backend summary.
- Launch screenshot and screenshots after narrowing and widening the root
  window.
- Before/after screenshots for every tooltip or label lane exercised.
- For a continuously open long tooltip, a five-second video or screenshots at
  0, 2, and 5 seconds proving its width does not shrink.
- Exact commands, PIDs, fixture paths, and any crash or rendering logs. Fixture
  paths may be redacted to their task-specific basename in the PR report.

## Build the exact PR binaries

Run these commands from the PR checkout:

```bash
git lfs pull
export HORIZON_SMOKE_PR_SHA="$(git rev-parse HEAD)"
test "$HORIZON_SMOKE_PR_SHA" = '<requested PR head SHA>'
git diff --exit-code

export HORIZON_SMOKE_ROOT="$(mktemp -d /tmp/horizon-issue-271-macos.XXXXXX)"
export HORIZON_SMOKE_CLEAN_HOME="$HORIZON_SMOKE_ROOT/clean-home"
export HORIZON_SMOKE_SEEDED_HOME="$HORIZON_SMOKE_ROOT/seeded-home"
export HORIZON_SMOKE_UPDATE_HOME="$HORIZON_SMOKE_ROOT/update-home"
export HORIZON_SMOKE_PERSIST_HOME="$HORIZON_SMOKE_ROOT/persist-home"
mkdir -p "$HORIZON_SMOKE_CLEAN_HOME" "$HORIZON_SMOKE_SEEDED_HOME" \
  "$HORIZON_SMOKE_UPDATE_HOME" "$HORIZON_SMOKE_PERSIST_HOME"

CARGO_TARGET_DIR="$HORIZON_SMOKE_ROOT/target-pr" cargo build
cp "$HORIZON_SMOKE_ROOT/target-pr/debug/horizon" "$HORIZON_SMOKE_ROOT/horizon-pr"
export HORIZON_SMOKE_PR_BIN="$HORIZON_SMOKE_ROOT/horizon-pr"
```

For the microphone-tooltip lane, build the same head with the macOS speech
feature and copy it before any other build:

```bash
CARGO_TARGET_DIR="$HORIZON_SMOKE_ROOT/target-pr-speech" cargo build --features speech
cp "$HORIZON_SMOKE_ROOT/target-pr-speech/debug/horizon" "$HORIZON_SMOKE_ROOT/horizon-pr-speech"
```

If the speech prerequisites are unavailable, record that lane as unavailable;
all other lanes still use `HORIZON_SMOKE_PR_BIN`. The current base has a known
manifest/lock mismatch in its Surge dependency, so Cargo refreshes only
`Cargo.lock` during either build. Restore that generated change, then confirm
the checkout is otherwise clean:

```bash
git restore --source=HEAD -- Cargo.lock
git diff --exit-code
```

## Baseline macOS/Metal launch and resize

Launch the clean ephemeral lane from outside the repository:

```bash
cd "$HORIZON_SMOKE_ROOT"
/usr/bin/env HOME="$HORIZON_SMOKE_CLEAN_HOME" \
  XDG_CONFIG_HOME="$HORIZON_SMOKE_CLEAN_HOME/.config" \
  "$HORIZON_SMOKE_PR_BIN" --ephemeral --blank
```

1. Record the exact PID and identify root and detached windows by PID rather
   than application name.
2. Confirm the adapter log reports Metal and use a Retina-scaled display when
   available.
3. Capture the visible, responsive blank launch state. In the empty-state card,
   click New Terminal, then choose `Skip (use default)` if the directory picker
   appears. Confirm this creates a workspace, shell panel, and minimap.
4. Resize narrower and wider. Confirm the new panel chrome,
   workspace/minimap labels, and toolbar controls remain visible without
   overlap, then capture both resized states.

## Reproducible attention, SSH, title, and speech fixture

The attention badge needs a real unresolved `AttentionItem`; an OSC
notification is not sufficient because notification-only attention is resolved
during the same board update. The fixture below instead exercises Horizon's
real failed-restore path. Its panel name starts after the 19-byte
`Failed to restore '` prefix, so nine ASCII characters place `å` across the old
`summary[..29]` byte boundary. A second failed restore starts with wide CJK
glyphs to verify measured badge width and the compact narrow-panel fallback
independently of the character budget.

Create the fixture directories:

```bash
export HORIZON_SMOKE_REMOTE="$HORIZON_SMOKE_ROOT/remote-target-å-😊"
mkdir -p "$HORIZON_SMOKE_SEEDED_HOME/.horizon" \
  "$HORIZON_SMOKE_ROOT/fixture-bin" "$HORIZON_SMOKE_REMOTE"
```

Create `$HORIZON_SMOKE_SEEDED_HOME/.horizon/config.yaml` with exactly:

```yaml
version: 8
features:
  attention_feed: true
  speech:
    enabled: true
    backend: metal
    input_device: ""
    hotkey_mode: hold
    profiles:
      - name: Norsk diktering med svært langt profilnavn
        model: /tmp/horizon-issue-271-unused.gguf
        language: "no"
        task: transcribe
        hotkey: F1
        preload: false
      - name: English dictation with another deliberately long profile name
        model: /tmp/horizon-issue-271-unused.gguf
        language: en
        task: transcribe
        hotkey: F2
        preload: false
      - name: Norsk til engelsk oversettelse med emoji 😊
        model: /tmp/horizon-issue-271-unused.gguf
        language: "no"
        task: translate
        target_language: en
        hotkey: F3
        preload: false
workspaces:
  - name: "Workspace first line\nWorkspace second line æøå 😊"
    terminals:
      - name: "aaaaaaaaaå😊 very long broken restore panel"
        kind: codex
        command: "bad\0codex"
      - name: "二二二二二二二二二二二二二二二二二二二二二二二二二二二二二二 wide badge"
        kind: codex
        command: "bad\0codex"
      - name: "Panel first line\nPanel second line æøå 😊"
        kind: shell
      - name: "SSH upload Unicode destination å 😊"
        kind: ssh
        ssh_connection:
          host: horizon-smoke-local
```

The fake model is never loaded because every `preload` is false and this lane
does not start dictation. F1, F2, and F3 are distinct valid bindings. A
non-speech binary safely ignores the runtime feature, but the microphone lane
requires the speech binary.

Create `$HORIZON_SMOKE_ROOT/fixture-bin/ssh` with exactly this local transport
shim, then run `chmod 700 "$HORIZON_SMOKE_ROOT/fixture-bin/ssh"`:

```sh
#!/bin/sh
batch=
last_arg=
for arg in "$@"; do
  [ "$arg" = BatchMode=yes ] && batch=1
  last_arg=$arg
done
if [ -n "$batch" ]; then
  : "${HORIZON_SMOKE_REMOTE:?}"
  HOME=$HORIZON_SMOKE_REMOTE
  export HOME
  exec /bin/sh -c "$last_arg"
fi
printf 'Horizon local SSH smoke fixture\n'
exec /bin/zsh -f
```

This shim is scoped to the test process through `PATH`. It gives the SSH panel
a local shell and executes Horizon's non-interactive directory probe/upload
command in the Unicode fixture directory; it never contacts an external host.

Choose the seeded binary and launch the config-backed ephemeral lane. Do not
pass `--blank`, because that would discard the configured fixture workspaces:

```bash
export HORIZON_SMOKE_SEEDED_BIN="$HORIZON_SMOKE_PR_BIN"
if test -x "$HORIZON_SMOKE_ROOT/horizon-pr-speech"; then
  export HORIZON_SMOKE_SEEDED_BIN="$HORIZON_SMOKE_ROOT/horizon-pr-speech"
fi
/usr/bin/env HOME="$HORIZON_SMOKE_SEEDED_HOME" \
  XDG_CONFIG_HOME="$HORIZON_SMOKE_SEEDED_HOME/.config" \
  HORIZON_SMOKE_REMOTE="$HORIZON_SMOKE_REMOTE" \
  PATH="$HORIZON_SMOKE_ROOT/fixture-bin:/usr/bin:/bin:/usr/sbin:/sbin" \
  "$HORIZON_SMOKE_SEEDED_BIN" --ephemeral
```

Record the exact PID separately from the baseline PID.

## Unicode truncation, labels, and badges

1. On seeded startup, confirm both broken-restore placeholders appear and their
   attention badges are visible. Confirm there is no panic at the former
   byte-29 boundary and the long summaries end in exactly one Unicode ellipsis.
   Narrow both panels: the CJK badge must remain inside its reserved titlebar
   slot without covering the title, history meter, microphone, or close button.
2. Confirm the seeded panel and workspace names containing `\n`, Norwegian
   characters, and emoji render as one elided line rather than stopping at the
   newline. Check vertical centering, grip spacing, titlebar controls, and
   minimap label placement.
3. In the normal shell panel, print a long Unicode line, open terminal search
   with Cmd+Shift+F, and search for a matching fragment. Confirm result detail
   text truncates without malformed characters.
4. Open the command palette with Cmd+Shift+K so its shortcut badges are
   visible. In Settings → General → Appearance, select Dark and then Light.
   Compare attention, minimap-label, search-count, and command-shortcut badge
   backgrounds in both themes. Their prior corner radii, fills, strokes,
   alignment, and clipping must remain visually unchanged.

## SSH upload Unicode lane

Create the source file outside Horizon:

```bash
export HORIZON_SMOKE_UPLOAD="$HORIZON_SMOKE_ROOT/unicode-upload-aaa😊-very-long.txt"
printf 'issue 271 ssh upload fixture\n' > "$HORIZON_SMOKE_UPLOAD"
open "$HORIZON_SMOKE_ROOT"
```

1. Confirm the seeded SSH panel displays `Horizon local SSH smoke fixture`.
2. Drag the exact Unicode source file from Finder onto the SSH terminal body.
   Wait for the local directory probe to finish.
3. Confirm the file pill is exactly `unicode-upload-aaa😊…`: one ellipsis
   inside the 20-scalar budget, with no replacement glyph or `...` sequence.
4. Leave the Unicode default destination, click Upload, and confirm success.
5. Prove the interaction and local transport completed:

   ```bash
   cmp "$HORIZON_SMOKE_UPLOAD" \
     "$HORIZON_SMOKE_REMOTE/$(basename "$HORIZON_SMOKE_UPLOAD")"
   ```

## Stable tooltip lanes

For every lane, keep the pointer stationary for at least five seconds while
Horizon redraws. For the Empty preset, detached-window, and minimap lanes,
confirm the single-line tooltip stays within the viewport, uses `…` when
needed, never narrows frame over frame, and never opens a nested native
tooltip. For the FPS, microphone, invalid-shortcut, and Update lanes, confirm
the bounded tooltip wraps at words, preserves every diagnostic line, and stays
the same width frame over frame.

- Toolbar FPS meter, including its idle state.
- Panel microphone: in the speech-enabled seeded process, hover the mic on the
  idle shell panel. The F1/F2/F3 profile names above create the long hotkey
  summary; do not click, request microphone permission, or load the fake model.
- Invalid shortcut: open Settings → Shortcuts, replace one binding with the
  literal invalid value `Ctrl++K`, then hover the red `!` indicator.
- Empty preset name: add/edit a preset in Settings, clear its name, and hover
  the red empty Name text field itself; that field owns the validation tooltip.
- Detached-window Fit Workspace and minimap shortcut buttons.
- Minimap target containing the seeded long Unicode workspace/panel label.

For the detached lanes, click `Detach` in the seeded workspace toolbar, record
the new native window as another window owned by the exact seeded PID, and run
the checks there. Close that child window afterward and confirm the workspace
reattaches to the root window.

Repeat the long-tooltip checks in a narrow root viewport and, where applicable,
a narrow detached viewport. Capture screenshots at 0, 2, and 5 seconds for at
least one long tooltip with a stationary pointer.

### Seed the multiline update tooltip and download error

Set up a managed-install-shaped copy of the exact PR binary:

```bash
case "$(uname -m)" in
  arm64) export HORIZON_SMOKE_RID=osx-arm64 ;;
  x86_64) export HORIZON_SMOKE_RID=osx-x64 ;;
  *) printf 'unsupported macOS architecture\n' >&2; exit 1 ;;
esac
export HORIZON_SMOKE_UPDATE_ROOT="$HORIZON_SMOKE_ROOT/managed-install"
mkdir -p "$HORIZON_SMOKE_UPDATE_ROOT/app/.surge" "$HORIZON_SMOKE_ROOT/no-open-bin"
cp "$HORIZON_SMOKE_PR_BIN" "$HORIZON_SMOKE_UPDATE_ROOT/app/horizon"
chmod 700 "$HORIZON_SMOKE_UPDATE_ROOT/app/horizon"
printf 'id: horizon-%s\nversion: 0.0.1\nchannel: stable\ninstallDirectory: horizon\nsupervisorId: smoke\nprovider: github_releases\nbucket: peters/horizon-updates\nregion: surge\nendpoint: ""\n' \
  "$HORIZON_SMOKE_RID" > "$HORIZON_SMOKE_UPDATE_ROOT/app/.surge/runtime.yml"
```

Launch it with a `PATH` that intentionally excludes `/usr/bin/open` while the
Rust HTTP update check remains available:

```bash
/usr/bin/env HOME="$HORIZON_SMOKE_UPDATE_HOME" \
  XDG_CONFIG_HOME="$HORIZON_SMOKE_UPDATE_HOME/.config" \
  PATH="$HORIZON_SMOKE_ROOT/no-open-bin:/bin:/usr/sbin:/sbin" \
  "$HORIZON_SMOKE_UPDATE_ROOT/app/horizon" --ephemeral --blank
```

1. With network access to GitHub Releases, wait for the Update button. The
   `0.0.1` runtime version guarantees the public stable channel reports a newer
   macOS package. If release metadata is unreachable, attach the log and mark
   only this externally dependent lane unavailable.
2. Hover Update and confirm the built-in newline-separated text remains fully
   visible as a bounded multiline tooltip.
3. Click Update. The missing `open` command must append
   `Last download attempt failed: failed to open installer download: No such
   file or directory (os error 2)`.
4. Hover again for five seconds and capture 0/2/5-second evidence for the long
   multiline-plus-error tooltip. Confirm the complete failure reason remains
   visible, the button receives one click, and the tooltip remains stable.

## Existing-state persistence and migration lane

This is a separate persistent lane: never pass `--ephemeral`. Build the exact
merge-base version in a detached task worktree so the fixture is genuinely
created by pre-PR code:

```bash
cd '<PR checkout>'
git fetch origin main
export HORIZON_SMOKE_BASE_SHA="$(git merge-base HEAD origin/main)"
export HORIZON_SMOKE_BASE_TREE="$HORIZON_SMOKE_ROOT/base-tree"
git worktree add --detach "$HORIZON_SMOKE_BASE_TREE" "$HORIZON_SMOKE_BASE_SHA"
git -C "$HORIZON_SMOKE_BASE_TREE" lfs pull
CARGO_TARGET_DIR="$HORIZON_SMOKE_ROOT/target-base" \
  cargo build --manifest-path "$HORIZON_SMOKE_BASE_TREE/Cargo.toml"
cp "$HORIZON_SMOKE_ROOT/target-base/debug/horizon" "$HORIZON_SMOKE_ROOT/horizon-base"
git -C "$HORIZON_SMOKE_BASE_TREE" restore --source=HEAD -- Cargo.lock
git -C "$HORIZON_SMOKE_BASE_TREE" diff --exit-code
git worktree remove "$HORIZON_SMOKE_BASE_TREE"
mkdir -p "$HORIZON_SMOKE_PERSIST_HOME/.horizon"
```

Create `$HORIZON_SMOKE_PERSIST_HOME/.horizon/config.yaml` with exactly:

```yaml
version: 8
workspaces:
  - name: Existing main workspace æøå
    terminals:
      - name: Existing main panel 😊
        kind: editor
```

1. Launch
   `/usr/bin/env HOME="$HORIZON_SMOKE_PERSIST_HOME" XDG_CONFIG_HOME="$HORIZON_SMOKE_PERSIST_HOME/.config" "$HORIZON_SMOKE_ROOT/horizon-base"`
   with no CLI flags. Double-click the workspace label and panel title, enter
   long Unicode names, and press Return after each edit. Wait at least one
   second for the 500 ms save debounce, then quit normally and wait for the
   exact PID to exit.
2. Locate the one `runtime.yaml` below
   `$HORIZON_SMOKE_PERSIST_HOME/.horizon/sessions/` and confirm it contains both
   renamed values. Save the SHA-256 hash of `config.yaml` before PR startup.
3. Launch
   `/usr/bin/env HOME="$HORIZON_SMOKE_PERSIST_HOME" XDG_CONFIG_HOME="$HORIZON_SMOKE_PERSIST_HOME/.config" "$HORIZON_SMOKE_PR_BIN"`
   with no flags. Confirm there is no migration prompt or migration log and the
   config checksum is unchanged. Do not require a byte-identical runtime file:
   normal window-geometry autosave may legitimately rewrite it.
4. Confirm the base-created names restore and elide correctly. Make one new
   rename, wait one second, quit normally, repeat the same PR-binary command
   with no flags, and confirm the new name persists semantically.

## Interaction and accessibility

1. Click each tested control after its tooltip has been open to confirm tooltip
   attachment did not consume or duplicate the interaction response.
2. Verify minimap accessibility text still names the hovered target.
3. Confirm no tested process targets or modifies a different Horizon PID.

## Pass criteria and report

- No panic, malformed UTF-8, tooltip shrink-collapse, nested tooltip, badge
  style regression, clipping regression, broken click, or unexpected config
  migration.
- Launch and resize screenshots show a fully rendered macOS/Metal UI.
- Automated and manual observations were made on the exact requested PR head.

Post the results to the PR as:

```text
SMOKE-TEST REPORT (macOS)
- <step>: pass | fail — short note
- ...
Summary: <fixes pushed, remaining issues, and evidence locations>
SMOKE-TEST: DONE
```

After every lane passes, delete this temporary plan, push that cleanup to the
same PR branch, and include the resulting head SHA in the report. The final line
must be exactly `SMOKE-TEST: DONE`.
