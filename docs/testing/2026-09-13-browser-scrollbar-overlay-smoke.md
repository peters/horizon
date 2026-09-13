# Browser scrollbar overlay smoke

Temporary validation for host-owned vertical scrollbars in Horizon browser
panels. Chromium's native gutter is too low-contrast to use, and Firefox
screenshots omit native scrollbar pixels. Horizon now paints a dark track with
an accent thumb over every scrollable page and routes gutter presses through
engine-owned `scrollTo`.

Build and launch the exact candidate (`target/debug/horizon`) with an isolated
`--config` and `--ephemeral`. Unset `HORIZON` for the child. Do not reuse a
pre-existing Horizon process.

## Lanes

1. Chromium (default backend)
2. Firefox (`backend: firefox`)
3. Safari (`backend: safari`) — macOS only. `safaridriver` is unavailable on
   Linux and Windows; those agents must record this lane as skipped with that
   reason, not as a pass. On a Mac, run the same steps as Chromium/Firefox.

Use a tall deterministic page (local `data:` URL or `scroll.html` fixture) whose
root document is at least 3× the panel height.

## Steps (each backend)

1. Launch, create a Browser panel, wait until the first page frame is visible.
2. Confirm a vertical overlay is painted on the right edge of the page (dark
   track, accent thumb) without using the mouse wheel first.
3. Drag the thumb from the top toward the bottom. The page must scroll, and the
   thumb must follow. Record `scrollY` before and after (page console or
   `browser_evaluate` `window.scrollY`).
4. Click the overlay track below the thumb. The page must page down by roughly
   one viewport.
5. Click content just left of the overlay. That click must hit the page, not
   scroll.
6. Wheel over the page body. Wheel scrolling must still work.
7. Resize the panel, then repeat step 3 once.
8. Navigate to a short page that does not overflow. The overlay must disappear.
9. Switch Chromium → Firefox (or the reverse) on the same panel and repeat
   steps 2–6 on the tall page without a manual extra resize.
10. On macOS, also switch to Safari and repeat steps 2–8.

## Pass

- Overlay visible on a tall page before any wheel input on every executed
  backend (Chromium, Firefox, and Safari on macOS).
- Thumb drag and track click move `scrollY`.
- Content immediately left of the overlay still receives clicks.
- Wheel still works.
- Overlay gone on a non-overflowing page.
- Safari skipped on Linux/Windows with the platform reason recorded.

## Fail

- Overlay missing on Firefox (blank right strip only).
- Chromium overlay missing or undraggable (wheel is the only way to scroll).
- Safari overlay missing or undraggable on macOS.
- Track/thumb clicks stolen by panel resize or sent into page content.
