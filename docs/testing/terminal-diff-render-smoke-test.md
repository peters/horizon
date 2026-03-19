# Terminal Diff Render Smoke Test

## Goal

Verify that Horizon's terminal renderer keeps foreground glyphs and decorations visible when ANSI background colors change at the end of a text run. The regression showed up while viewing colored git diffs in Codex, where trailing highlighted segments could render as solid color blocks with missing text.

## Test Environment

- Worktree: `/tmp/horizon-diff-render-fix`
- Display server: active X11 session on `DISPLAY=:1`
- Binary: `target/debug/horizon`
- Use an isolated config path so local presets do not affect startup

## One-Time Setup

Run these commands in a separate shell before launching Horizon:

```bash
rm -f /tmp/horizon-diff-render-empty.yaml
rm -rf /tmp/horizon-diff-render-repro
mkdir -p /tmp/horizon-diff-render-repro
cd /tmp/horizon-diff-render-repro

git init

cat > demo.txt <<'EOF'
alpha
beta
gamma
delta
EOF

git add demo.txt
git -c user.name='Test User' -c user.email='test@example.com' commit -m 'init'

cat > demo.txt <<'EOF'
alpha
BETTA change
gamma delta epsilon
delta
EOF

cat > wide.txt <<'EOF'
plain ascii line
tabs	and	spacing
emoji cafe
EOF

git add wide.txt

cat > wide.txt <<'EOF'
plain ascii line
tabs	and	SPACING
emoji cafe 你好
EOF
```

## Launch

Start Horizon from the worktree:

```bash
cd /tmp/horizon-diff-render-fix
cargo build -p horizon-ui
DISPLAY=:1 target/debug/horizon --blank --ephemeral --config /tmp/horizon-diff-render-empty.yaml
```

Expected result:

- Horizon opens a single terminal panel.
- No startup chooser appears.
- No local user config is loaded.

## Baseline Checks

Inside the Horizon terminal, run:

```bash
pwd
printf 'plain terminal output\n'
```

Verify:

- Plain terminal text renders normally.
- Cursor is visible.
- Selection works.
- Scrollbar appears and tracks scrollback.

## Minimal ANSI Regression Repro

Inside the Horizon terminal, run this exact command:

```bash
printf $'\033[2J\033[H\033[39mAAAAAAAAAA\033[41mBBBBBBBBBB\033[0m\n\033[39mCCCCCCCCCC\033[42mDDDDDDDDDD\033[0m\n'
```

Expected result on the fixed build:

- The trailing red segment shows ten visible `B` glyphs on a red background.
- The trailing green segment shows ten visible `D` glyphs on a green background.
- No trailing colored segment turns into a solid block with the text missing.

Expected failure on the broken build:

- The trailing `BBBBBBBBBB` or `DDDDDDDDDD` segment may appear as a flat red or green rectangle with the glyphs hidden under the background fill.

## Git Diff Repro

Inside the Horizon terminal, run:

```bash
cd /tmp/horizon-diff-render-repro
git diff --color=always
git diff --color=always --word-diff=color
```

Verify:

- Added and removed lines remain readable across the full line.
- Intraline highlighted segments stay readable.
- Highlighted segments at the end of a line do not lose their text.

## Edge Cases

Inside the Horizon terminal, run:

```bash
git diff --color=always --word-diff=color -- wide.txt
printf 'underline: \033[4mUNDERLINED\033[0m strike: \033[9mSTRUCK\033[0m\n'
```

Verify:

- Tabs and wide characters do not break neighboring colored segments.
- Underline and strikeout remain visible on top of any cell background.

## Interaction Checks

1. Resize the Horizon window larger, then smaller.
2. Re-run the minimal ANSI repro command.
3. Re-run `git diff --color=always --word-diff=color`.
4. Scroll up and down through the diff.
5. Drag-select text across plain cells and colored cells.

Verify:

- The same colored segments remain readable after resize.
- Scrolling does not introduce missing text.
- Selection remains legible over diff colors.

## Persistence Check

1. Close Horizon.
2. Relaunch with the same command.
3. Re-run the minimal ANSI repro command.

Verify:

- Rendering is still correct after restart.
- No startup-state regression appears from the isolated config path.

## Screenshot Evidence

Capture:

1. A screenshot immediately after launch.
2. A screenshot after the minimal ANSI repro command.
3. A screenshot after `git diff --color=always --word-diff=color`.
4. A screenshot after the resize-and-rerun pass.

If using shell automation on the same machine, these commands are sufficient once the Horizon window id is known:

```bash
DISPLAY=:1 xdotool search --name '^Horizon$'
DISPLAY=:1 import -window <window-id> /tmp/horizon-diff-render-shot.png
```

## Pass Criteria

- Foreground glyphs are always painted above colored cell backgrounds.
- Git diff output is readable in both line-level and word-diff color modes.
- Trailing colored segments do not disappear at the end of a text run.
- Plain terminal text, selection, cursor, and scrolling still behave normally.
