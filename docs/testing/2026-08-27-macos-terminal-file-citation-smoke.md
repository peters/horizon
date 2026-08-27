# macOS terminal file-citation smoke plan (temporary)

## Scope

Validate PR #318 on macOS. Supported file-citation tokens should render as
compact chips in the supported agent terminal, while ordinary terminals and
unsupported inputs keep their literal text. This PR is display-only: it must
not add click-to-open behavior.

Run every step from the PR worktree at the exact requested commit. Use the
debug binary for faster iteration.

## Preconditions

- macOS 14 or newer, Metal-capable Mac, and Xcode Command Line Tools.
- No uncommitted changes in the smoke checkout.
- Record `git rev-parse HEAD` in the report and confirm it equals the current
  PR head before testing.
- Do not stop or automate any pre-existing Horizon process. Track the exact
  PID launched for this smoke and close only that process through the normal
  window close path.

## Build and automated checks

```bash
cargo build
cargo test -p horizon-core terminal::file_citation --lib
cargo test -p horizon-ui terminal_widget::file_citation
```

Expected: the build and all focused tests pass.

## Isolated fixture

Create a temporary directory outside the repository, including an empty
`home` subdirectory. Put this executable `emit-citations.sh` inside it and run
`chmod +x <SMOKE_DIR>/emit-citations.sh`:

```bash
#!/bin/bash
printf '\033[1;36mCitation rendering smoke\033[0m\r\n\r\n'
printf 'Source: :codex-file-citation{path="/tmp/guides/Linux install.pdf" purpose="source"}\r\n\r\n'
printf 'Output: :codex-file-citation{path="/tmp/reviewed report.docx" purpose="output"}\r\n\r\n'
printf 'Wrapped pair: :codex-file-citation{path="/tmp/very-long-folder/Baxi.AgentXMLInterface_v1.6.6.pdf" purpose="source"} :codex-file-citation{path="/tmp/very-long-folder/Baxi.Agent_Errorcodes.pdf" purpose="source"}\r\n\r\n'
printf 'Malformed then valid: :codex-file-citation{broken :codex-file-citation{path="/tmp/recovered.pdf" purpose="source"}\r\n'
printf 'Unsupported: :codex-file-citation{path="/tmp/preview.pdf" purpose="preview"}\r\n'
printf 'Unicode literal: :codex-file-citation{path="/tmp/résumé.pdf" purpose="source"}\r\n'
printf 'Decomposed literal: :codex-file-citation{path="/tmp/resume\314\201.pdf" purpose="source"}\r\n'
printf 'Exact spaces: :codex-file-citation{path=" /tmp/report.pdf " purpose="output"}\r\n'
exec /bin/bash --noprofile --norc
```

Create `config.yaml` beside it, replacing both `<SMOKE_DIR>` values with the
absolute temporary-directory path:

```yaml
version: 9
window:
  width: 1280
  height: 800
appearance:
  theme: dark
features:
  attention_feed: false
workspaces:
  - name: Citation Smoke
    position: [40, 40]
    terminals:
      - name: Supported panel
        kind: codex
        command: /bin/bash
        args: [--noprofile, --norc, <SMOKE_DIR>/emit-citations.sh]
        position: [60, 80]
        size: [720, 560]
      - name: Ordinary shell
        kind: shell
        command: /bin/bash
        args: [--noprofile, --norc, <SMOKE_DIR>/emit-citations.sh]
        position: [800, 80]
        size: [360, 560]
```

Launch without inheriting another Horizon session:

```bash
env -u HORIZON HOME=<SMOKE_DIR>/home target/debug/horizon \
  --config <SMOKE_DIR>/config.yaml --ephemeral
```

Record the exact PID. Take a launch screenshot and inspect it before relying
on it as evidence.

## Visual and interaction checks

1. In `Supported panel`, valid `source` and `output` tokens render as cyan
   chips with readable `Source · <name>` and `File · <name>` labels.
2. The wrapped pair produces stable chips without overlapping adjacent text,
   painting outside the panel, or leaving fragments of valid markup visible.
3. The valid `recovered.pdf` token after the malformed candidate becomes a
   chip. The malformed prefix remains literal.
4. Unsupported-purpose, composed-Unicode, and decomposed-Unicode tokens stay
   literal and readable. They must not disappear or become partial chips.
5. The exact-spaces token becomes a chip; surrounding path whitespace must
   not make the token malformed.
6. In `Ordinary shell`, every token remains literal. No chip is painted.
7. Drag-select across one valid citation in `Supported panel`. While selected,
   the original token is visible; copying and pasting into the shell reproduces
   the token rather than the chip label.
8. Plain-click a chip location. This display-only PR must not open Finder,
   Preview, a browser, or another application. Do not Command-click paths;
   existing generic terminal path handling is outside this PR.
9. Resize the supported panel from wide to its practical minimum and back,
   then use fit-workspace. Labels may elide, but token content must never become
   invisible: a chip or literal fallback must remain, with no clipping crash.
10. Switch to the light theme and back to dark. Chip text and border remain
    legible, with no stale cached colors.
11. Move the pointer over the panel and leave Horizon idle for 30 seconds.
    There should be no continuous animation, visible jitter, or obvious new
    idle-CPU spike. Terminal output and selection should still refresh chips.
12. Close the exact smoke window normally, confirm its PID exits, and relaunch
    once with the same isolated config. Repeat checks 1, 3, and 6 to catch
    restore/cache regressions.

Capture a final screenshot after resize/fit and inspect it. Include the launch
and final screenshot paths in the report; do not commit them.

## Report and cleanup

Remove the temporary directory after the report. Reply on PR #318 using:

```text
SMOKE-TEST REPORT (macOS)
- exact PR head: pass | fail — <sha>
- build and focused tests: pass | fail — <note>
- supported-panel rendering: pass | fail — <note>
- malformed and unsupported fallback: pass | fail — <note>
- ordinary-shell isolation: pass | fail — <note>
- selection/copy and plain-click behavior: pass | fail — <note>
- resize, fit, themes, and relaunch: pass | fail — <note>
- evidence: <screenshot paths or attached images>
Summary: <fixes pushed, remaining failures, or clean pass>
SMOKE-TEST: DONE
```

The final line must be exactly `SMOKE-TEST: DONE`.
