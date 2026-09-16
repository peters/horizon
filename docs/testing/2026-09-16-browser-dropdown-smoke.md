# Browser native select overlay smoke

Temporary validation for issue 714. Chromium `Page.startScreencast` and Firefox
`WebDriver` screenshots omit the native popup for a size-1 `<select>`, and
protocol pointer events cannot hit that popup. Horizon paints a host-owned
menu from a page probe and applies the chosen option through `input`/`change`
events. Custom DOM dropdowns and in-page listboxes (`multiple` / `size>1`)
stay on the normal page input path.

Build and launch the exact candidate (`target/debug/horizon`) with an isolated
`--config` and `--ephemeral`. Unset `HORIZON` for the child. Do not reuse a
pre-existing Horizon process.

Serve `scripts/browser-smoke/fixtures/dropdowns.html` with
`scripts/browser-smoke/fixture_server.py` (or the reusable runner) and navigate
the panel to that local URL.

## Lanes

Record an explicit pass/fail or unsupported result per backend. Do not infer
Firefox from Chromium.

1. Chromium (default backend)
2. Firefox (`backend: firefox`)
3. Safari (`backend: safari`) — macOS only. Record skipped on Linux/Windows.

Record browser versions.

## Steps (each backend)

Capture evidence while a menu is open and after selection. Inspect screenshots.
Record a short interaction sequence when still images cannot establish the
behavior.

### Native single-select

1. Click `#native-single`. A visible option menu must open over the page.
2. Choose **Charlie**. The closed control and `value=` line must show `charlie`.
   The log must record `input` and `change`.
3. Reopen, confirm the highlighted/selected item is Charlie, choose **Alpha**.

### Native options

4. Open `#native-grouped`. Group headers must be visible. **Nope** must not be
   selectable. Choose **Two**.
5. Open `#native-long`. Scroll the menu and choose **Item 15**.
6. Click `#native-multi` (in-page listbox). Select **Green** without a host
   overlay. The listbox itself must remain the control.

### Keyboard

7. Tab to `#native-single`. Space or Alt+ArrowDown (platform-appropriate) must
   open the host menu. Arrow to another enabled option, Enter to select.
8. Reopen, press Escape. The menu must close without changing the value. Focus
   must remain on the select.
9. Reopen, type `c` to jump toward Charlie, Enter to select.

### Custom DOM combobox

10. Click **Choose fruit**. The in-page list (Apple/Pear/Plum) must open from
    the page, not as the host native overlay.
11. Choose **Pear**. `value=pear`. Click outside an open list to dismiss.

### Geometry

12. Open `#edge-select` at the right edge. The host menu must stay visible and
    clickable inside the panel.
13. Scroll the page, reopen a select, choose an option. Click targeting must
    still match the visible menu.
14. Resize/Fit the panel with a menu open. The menu must dismiss or retarget
    without a stuck overlay. Reopen and select after resize.
15. If the panel can zoom, reopen and select under that scale.

### Lifecycle

16. Open a menu, click another panel or the canvas. The menu must close. The
    other panel must receive the click.
17. Open a menu, navigate the same panel away. No stuck overlay or input
    capture on the next page.
18. Repeated open/close of `#native-single` must not leave a ghost menu.

## Notes

- Setting `select.value` from the console is not evidence. The menu must be
  visible and the option must be chosen with mouse or keyboard.
- Trusted `click` on the `<select>` is expected. Applied `change` events from
  the host overlay are script-dispatched.
