# Smoke-Test Plan - Settings YAML selection vs intersecting terminal (temporary)

> Validation artifact for the settings-YAML / terminal-selection fix. Delete
> after the UI validation pass is complete.

Selecting text in the settings YAML editor must not also select text in a
terminal panel whose unclipped canvas rect intersects the settings overlay.
The canvas already clips painting to the settings-free region; this PR makes
terminal pointer hit-testing honor that same clip.

Executable without extra context on any machine with a debug build:

```bash
cargo build
target/debug/horizon --config <plan-board>.yaml --ephemeral
```

Synthetic board (save as `<plan-board>.yaml`):

```yaml
version: 10
workspaces:
  - name: YAML Selection
    terminals:
      - name: notes
        kind: shell
        command: /bin/bash
        args: ["--noprofile", "--norc"]
        position: [40, 40]
        size: [1400, 720]
```

Use a wide terminal so it still occupies the canvas after Settings opens and
visually continues under the right-hand YAML panel. Fill the terminal with
unique lines before selecting YAML, for example:

```bash
python3 -c 'for i in range(1, 80): print(f"TERM-MARK-{i:03}")'
```

On headless Linux: `Xvfb :99 -screen 0 1600x1000x24`, launch with
`DISPLAY=:99`, screenshot via `import -window root`, drive with `xdotool`.
Scope automation to the exact Horizon PID under test.

## Baseline

1. Launch with the board above. Expect: one wide shell panel on the canvas,
   toolbar Settings button, no YAML overlay yet.
2. Open Settings (toolbar button or the configured shortcut). Expect: right
   settings panel plus bottom settings bar; canvas shrinks; the terminal is
   clipped at the settings edge rather than painting over YAML.
3. Switch to the YAML tab. Expect: syntax-highlighted `config.yaml` editor
   occupying the settings panel.

## Primary flow

4. Click in the YAML editor and drag a multi-line selection down through
   several keys (for example a `speech.profiles` block). Expect:
   - YAML text highlights in the editor.
   - The intersecting terminal does **not** gain a selection highlight.
   - Terminal contents stay fully readable with no block overlay matching the
     YAML drag's Y-range.
5. Copy the YAML selection (Ctrl/Cmd+C) and paste into the terminal. Expect:
   only the YAML text is pasted; no terminal cell range is copied.
6. Click in the **visible** terminal body (left of the settings edge) and drag
   a normal selection across a few `TERM-MARK-` lines. Expect: terminal
   selection works, YAML selection is cleared or ignored, copy from the
   terminal yields those `TERM-MARK-` lines.

## Edge cases

7. Drag-select YAML starting near the left edge of the editor (the settings /
   canvas boundary). Expect: still no terminal selection.
8. Drag-select YAML, then keep holding and move the pointer left into the
   visible terminal. Expect: YAML keeps its editor selection; the terminal
   must not start a new selection from that overlay-origin press.
9. Resize the settings panel wider and narrower, then repeat step 4. Expect:
   the clip follows the live settings width; YAML drags never select the
   terminal.
10. Close Settings. Drag-select in the now-unclipped terminal region that was
    previously under YAML. Expect: selection works across the full panel.
11. Re-open Settings on the General / Shortcuts / Presets tabs and drag in
    those GUI controls. Expect: no terminal selection. Return to YAML and
    confirm step 4 still holds.

## Persistence / migration

12. With Settings closed, select terminal text, then open Settings YAML and
    drag in the editor. Expect: the old terminal selection is not extended by
    the YAML drag (it may remain until a later in-terminal click, but a YAML
    drag must not grow or relocate it).
13. Relaunch `--ephemeral` with the same board. Expect the same clip and
    selection isolation; no session restore is required for this bug.

## Visual regression

14. Screenshot after step 4: YAML highlight confined to the settings panel;
    terminal has no matching selection overlay.
15. Screenshot after step 6: terminal selection is visible only in the canvas
    region, not under the YAML editor.
