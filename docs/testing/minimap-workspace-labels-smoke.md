# Smoke-test plan: minimap workspace labels + hover tooltip

Temporary validation artifact for the minimap label/tooltip rework. Delete after
all smoke lanes report done.

## Scope

- Rotated (top-to-bottom) workspace name labels in the minimap for tall, narrow
  workspace columns, replacing the old one-character-per-line stacks.
- Horizontal badge labels still used whenever the full name fits the title strip.
- Minimap hover tooltip no longer collapses into a glyph-wide vertical strip
  during long hovers or when moving between hover targets.

## Setup (any platform)

1. Build: `cargo build` (debug is fine; use `target/debug/horizon`).
2. Write a synthetic config that produces ~12 tall narrow workspace columns,
   one of them containing a panel with a long title:

   ```bash
   python3 - <<'EOF'
   names = [
       "Privat", "github.com/peters", "youpark.no", "Youpark Blomsterpike",
       "Bod", "Gulvvarme", "Horizon", "opera-omnia", "nativesdk-oe",
       "youpay-v2", "Dnb", "anpr.classifier",
   ]
   lines = ["version: 8", "workspaces:"]
   for i, name in enumerate(names):
       lines.append(f"  - name: {name!r}")
       lines.append(f"    position: [{i * 260.0}, 40.0]")
       lines.append("    terminals:")
       title = ("docker compose up — peters@peters:~/github/youpark.no"
                if name == "youpark.no" else f"notes-{i}")
       lines.append(f"      - name: {title!r}")
       lines.append("        kind: editor")
       lines.append("        position: [0.0, 0.0]")
       lines.append("        size: [220.0, 400.0]")
       lines.append(f"      - name: 'scratch-{i}'")
       lines.append("        kind: editor")
       lines.append("        position: [0.0, 420.0]")
       lines.append("        size: [220.0, 400.0]")
   open("/tmp/minimap-smoke-config.yaml", "w").write("\n".join(lines) + "\n")
   EOF
   ```

3. Launch without touching the real session store:
   `target/debug/horizon --config /tmp/minimap-smoke-config.yaml --ephemeral`

## Checks

### 1. Rotated labels (baseline)

- Locate the minimap (bottom-right overlay).
- PASS: every workspace column shows its name as smoothly rotated text reading
  top-to-bottom (book-spine style), one glyph run — not a stack of individual
  characters, not clipped mid-glyph horizontally.
- PASS: names longer than the column height end with an ellipsis rather than
  overflowing the column or the minimap frame.
- PASS: badges stay inside the minimap frame; no label paints over the
  neighbouring column's badge (collision suppression may hide a label of an
  inactive workspace instead — that is expected).

### 2. Horizontal labels still win where they fit

- Close or ignore the synthetic board; on any board with one or two wide
  workspaces (or zoom the synthetic board's minimap by resizing the window very
  wide), confirm wide workspace rects show the familiar horizontal badge in the
  title strip, not rotated text.

### 3. Tooltip stability (regression for the collapse bug)

- Hover a short-named target in the minimap (e.g. the `Dnb` column) for ~2 s;
  the tooltip shows compact text near the pointer.
- Move directly onto the `youpark.no` column's first panel (long title) and keep
  the pointer moving slightly within it for ~10 s.
- PASS: the tooltip shows `docker compose up — peters@peters:~/github/youpark.no`
  on a single line the entire time.
- FAIL (old bug): the tooltip renders as a tall, ~2-characters-per-line vertical
  strip, or narrows progressively while hovered.
- Move between several panels/workspaces quickly; the tooltip must re-anchor and
  resize per target with no residual narrow layouts.

### 4. Interaction unchanged

- Single-click a panel rect in the minimap: it focuses that panel.
- Single-click a workspace body: it focuses the workspace.
- Double-click a workspace: viewport fits that workspace.
- Drag inside the minimap: viewport pans.
- Hovering still shows the pointing-hand cursor; labels do not intercept clicks.

### 5. Active workspace emphasis

- The active workspace's label uses the brighter text/border treatment and its
  label is never suppressed by overlap with inactive labels.

### 6. Resize / fit

- Resize the window smaller until the minimap columns get very thin.
- PASS: labels degrade gracefully (ellipsize, then disappear below ~8 px column
  width); no panics, no labels escaping the minimap frame.

## Platform lanes

- Linux/X11: executed by the implementing agent (Xvfb + xdotool + screenshots).
- macOS/Metal: run checks 1, 3, and 6 on a Retina display (pixels_per_point = 2)
  to confirm rotated glyph rasterization is crisp and the tooltip behaves the
  same under winit/macOS pointer events.
