# Smoke-test plan: minimap workspace labels + hover tooltip

Temporary validation artifact for the minimap label/tooltip rework. Delete after
all smoke lanes report done.

## Scope

- Rotated (top-to-bottom) workspace name labels in the minimap for tall, narrow
  workspace columns, replacing the old one-character-per-line stacks.
- Horizontal badge labels whenever the measured name fits the title strip.
- Minimap hover tooltip no longer collapses into a glyph-wide vertical strip
  during long hovers or when moving between hover targets.

## Setup (any platform)

1. Build: `cargo build` (debug is fine; use `target/debug/horizon`).
2. Write a synthetic config that produces 12 tall narrow workspace columns,
   one of them containing a panel with a long title:

   ```bash
   python3 - <<'EOF'
   names = [
       "scratch", "github.com/acme", "parkside.dev", "Parkside Storefront",
       "storage", "heating-ctl", "horizon", "omnibus", "nativesdk",
       "payments-v2", "ledger", "plate.classifier",
   ]
   lines = ["version: 8", "workspaces:"]
   for i, name in enumerate(names):
       lines.append(f"  - name: {name!r}")
       lines.append(f"    position: [{i * 260.0}, 40.0]")
       lines.append("    terminals:")
       title = ("docker compose up — dev@buildbox:~/github/parkside.dev"
                if name == "parkside.dev" else f"notes-{i}")
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

Note: the minimap's size comes from `overlays.minimap_width`/`minimap_height`
in the config (defaults 320x180), and column width comes from how much board
content that fixed map must cover. Resizing the window changes neither; vary
column width via the config knobs below instead.

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

- Relaunch with a variant config containing two wide workspaces (three panels
  side by side: `position: [0,0]/[560,0]/[1120,0]`, `size: [520, 380]`)
  alongside two narrow columns from the base fixture.
- PASS: the wide workspace rects show the horizontal badge in the title strip
  with the full name; the narrow columns still use rotated labels.

### 3. Tooltip stability (regression for the collapse bug)

- Hover a short-named target in the minimap (e.g. the `ledger` column) for ~2 s;
  the tooltip shows compact text near the pointer.
- Move directly onto the `parkside.dev` column's first panel (long title) and
  keep the pointer moving slightly within it for ~10 s.
- PASS: the tooltip shows `docker compose up — dev@buildbox:~/github/parkside.dev`
  on a single line the entire time (elided with `…` only if it exceeds the
  tooltip width budget).
- FAIL (old bug): the tooltip renders as a tall, ~2-characters-per-line vertical
  strip, or narrows progressively while hovered.
- Sweep the pointer quickly across many panels and workspaces.
- PASS: the tooltip stays continuously visible while sweeping (no blank frames,
  no fade-in restart per target) and re-anchors per target with no residual
  narrow layouts.

### 4. Interaction unchanged

- Single-click a panel rect in the minimap: it focuses that panel.
- Single-click a workspace body: it focuses the workspace.
- Double-click a workspace: viewport fits that workspace.
- Drag inside the minimap: viewport pans.
- Hovering still shows the pointing-hand cursor; labels do not intercept clicks.

### 5. Active workspace emphasis

- The active workspace's label uses the brighter text/border treatment and its
  label is never suppressed by overlap with inactive labels.

### 6. Narrow-column degradation

- Relaunch twice with the base fixture edited to stress column width:
  a. 24 workspaces instead of 12 (halves each column's minimap width);
  b. base 12 workspaces plus `overlays:\n  minimap_width: 200.0` at the top of
     the config (narrows every column further).
- PASS: as columns shrink, labels first shorten to an ellipsized rotated run,
  then disappear entirely once a column cannot fit one glyph line across its
  width (roughly below ~11 px); no bare-`…` badges, no glyphs sliced
  lengthwise, no panics.

### 7. Detached workspace minimap

- Detach one workspace into its own window (workspace title-bar menu →
  Detach).
- PASS: the detached window's minimap shows that workspace's label with the
  active treatment, oriented sensibly for its (now much wider) rect, and its
  hover tooltip behaves as in check 3.
- Re-attach afterwards and confirm the main minimap recovers its label.

### 8. Persistence

- Labels and tooltips are paint-time only; nothing about them persists. After
  the detach/re-attach in check 7, quit and relaunch with the same config to
  confirm the board comes back and the minimap paints normally — no further
  persistence coverage is applicable to this change.

## Platform lanes

- Linux/X11: executed by the implementing agent (Xvfb + xdotool + screenshots).
- macOS/Metal: run checks 1, 3, 6, and 7 on a Retina display
  (pixels_per_point = 2) to confirm rotated glyph rasterization is crisp, the
  ellipsis-not-bare-`…` rule holds under physical-pixel glyph metrics, and the
  tooltip behaves the same under winit/macOS pointer events.
