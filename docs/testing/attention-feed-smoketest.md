# Attention Feed Smoke Test

Use this checklist before merging changes that affect the attention feed or its default enablement.

## Setup

- Launch Horizon with no `features.attention_feed` override in the config to verify the default path.
- Use one `Codex` panel and one `Claude` panel with deterministic scripted output so both agent kinds are exercised without depending on external auth state.
- For settings/minimap interaction, run the same smoke on a local checkout that also has PR `#33` applied.

## Core Feed

- Confirm the attention feed appears without any manual config toggle.
- Confirm separate `Codex` and `Claude` items appear when both panels emit attention states.
- Confirm an approval prompt (`Allow ...`, `[y/N]`, `(y/n)`) renders as a high-severity item.
- Confirm a question prompt ending in `?` renders as `Waiting for input`.
- Confirm a prompt line starting with `>` or `❯` renders as `Ready for input`.
- Confirm the feed orders newer items above older items.
- Confirm more than ten feed items truncates to the newest ten.

## Item Lifecycle

- Click `Go to panel` and verify focus jumps to the matching panel and workspace.
- Click dismiss on an open item and verify it disappears and stays dismissed.
- Resolve a prompt by emitting non-attention output and verify the item switches to resolved styling.
- Wait 30 seconds after resolution and verify the resolved item disappears automatically.
- Confirm dismissed items do not return unless the panel emits a new attention signal.

## Layout

- Verify the feed remains visible and usable on a wide window.
- Resize to a narrow window and verify the feed stays anchored and readable.
- With PR `#33` applied locally, open settings and verify the feed hides instead of overlapping settings.
- With PR `#33` applied locally, close settings and verify the feed returns in the correct corner.
- Verify feed placement remains correct when the minimap is visible and when it is hidden.

## Config

- Add `features.attention_feed: false`, reload config, and verify the feed and sidebar attention badges disappear.
- Remove the override, reload config, and verify the default-enabled feed returns.
