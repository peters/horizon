# Settings Overlay Smoke Test

Use this checklist for issue `#14` and any future work that changes the settings panel, minimap, or attention feed layout.

## Setup

Launch Horizon with:

- `features.attention_feed: true`
- at least one workspace so the minimap is visible
- at least one notification or attention item so the attention feed is visible
- one non-terminal panel if you want to use global shortcuts during the smoke pass

## Required Matrix

Capture live screenshots for all four states:

1. Wide window, settings closed
2. Wide window, settings open
3. Narrow window, settings closed
4. Narrow window, settings open

Suggested sizes:

- Wide: about `1480x920`
- Narrow: about `980x780`

## Expected Results

For `settings closed`:

- minimap is visible
- attention feed is visible when attention items exist
- neither overlay is clipped off-screen
- neither overlay overlaps the settings button or toolbar chrome

For `settings open`:

- settings editor is fully visible
- minimap is not visible
- attention feed is not visible
- canvas panels and workspace chrome do not render under the settings side panel
- the bottom settings action bar does not overlap canvas-space widgets

## Edge Cases

Check these before signing off:

- open settings from a wide window, then resize to narrow while settings stays open
- close settings after resizing back and forth at least once
- repeat the open/close cycle more than once to confirm overlays return consistently
- verify sidebar shown and sidebar hidden states if the touched code changes canvas reservation math
- if using notification-based attention items, remember they can age out of the feed after their resolved-item grace window; regenerate attention before blaming layout
