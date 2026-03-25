# Smoke Test: Kitty Printable Space Input Regression

This is a temporary validation artifact for the fix that keeps ordinary `Space`
input on the raw text path when kitty keyboard mode is enabled but
`REPORT_ALL_KEYS_AS_ESC` is not.

Delete this file after the live validation pass is complete unless it is
explicitly needed longer.

## Target

- Repository: `peters/horizon`
- Branch/PR: `fix/kitty-space-text-path`
- Primary platform: macOS
- Secondary platforms: Linux and Windows if convenient
- Scope: live typing feel and correctness inside a terminal panel

## Build Or Install

Choose one:

1. Build from source on the PR branch
   - Install Rust stable `>= 1.88`
   - Run `cargo run --release`
2. Use the PR build artifact if one is attached
   - Download the platform binary
   - Launch Horizon normally

## Test Setup

1. Start Horizon from a clean session if practical.
2. Open a single shell panel.
3. Click into the terminal body so it owns keyboard focus.
4. Use a shell prompt where spaces are easy to inspect, for example:
   - plain shell prompt
   - `python -q`
   - `cat -vet` or equivalent visible-character helper
5. If you can reproduce the original complaint, test with the same keyboard
   layout and the same terminal workload.

## Baseline Typing Checks

1. Press `Space` once.
   - Expected: one visible space appears.
   - Expected: the character appears immediately, without a noticeable pause.
2. Type `a b c`.
   - Expected: the terminal receives exactly `a b c`.
3. Hold `Space` briefly, then release.
   - Expected: one space appears.
   - Expected: no stuck modifier or delayed follow-up input.
4. Press `Shift+Space`.
   - Expected: one normal space appears.

## Primary Regression Checks

1. Type ten spaces quickly.
   - Expected: all ten spaces appear.
   - Expected: none are dropped.
   - Expected: typing cadence feels normal, not visibly slower than letters.
2. Type `a`, then five spaces, then `b`.
   - Expected: the exact number of spaces is preserved between `a` and `b`.
3. Type alternating letters and spaces quickly:
   - Example: `a a a a a`
   - Expected: every interleaved space appears.
4. Hold a letter key to generate repeat, then press `Space`.
   - Expected: the later space still appears once and in the correct order.
5. Press `Space`, then immediately another printable key.
   - Expected: input order is preserved.
   - Expected: the second key does not cause the space to disappear.

## Interaction And Shortcut Checks

1. Focus the terminal and type a few spaces, then use normal panel navigation.
   - Expected: the fix does not break terminal focus or input ownership.
2. If the platform uses the existing `Space + drag` canvas-pan gesture path,
   repeat one pan interaction after typing several spaces.
   - Expected: typing still works afterward.
   - Expected: the pan gesture still behaves as before.
3. Use `Cmd` or `Ctrl` modified shortcuts that normally bypass text input.
   - Expected: command shortcuts still win over raw text as before.

## Persistence And Relaunch

1. Type several spaces successfully.
2. Close Horizon cleanly.
3. Relaunch Horizon and open a shell panel again.
   - Expected: space handling remains correct after relaunch.
   - Expected: no stuck input state carries over.

## Visual And Evidence Checks

1. Capture a screenshot right after launch.
2. Capture a screenshot after a terminal line that clearly shows repeated spaces.
3. If typing feel is still suspect, capture a short screen recording while typing
   repeated spaces quickly.

## Report Back

Include the following in the PR comment or test report:

- Platform and OS version
- Keyboard layout used
- Whether the original slow-or-missing-space symptom reproduced
- Pass/fail for each primary regression check
- Launch screenshot and post-typing screenshot
- Any remaining input oddities found while testing
