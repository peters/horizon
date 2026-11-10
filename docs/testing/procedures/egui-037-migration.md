---
procedure: egui-037-migration
feature: UI dependency migration
platforms: [linux, macos]
cost: none
destructive: no
secrets: none
owner: peters
---

# egui 0.37 migration test procedure

## 1. Purpose

This procedure checks widget identity, wheel input, accessibility roles and
screenshot delivery after the egui update.

## 2. Applicability

- Candidate: Horizon with egui 0.37 and its API migration.
- Platforms: Linux and macOS.
- This procedure does not test paid remote browser providers.

## 3. Safety

Use synthetic terminal content only. Keep private content outside the isolated desktop and the receiver.

## 4. Equipment and preconditions

- Use the candidate checkout and Rust stable.
- Install the platform build prerequisites from `AGENTS.md`.
- Use the isolated desktop fixture from `scripts/device-smoke/README.md`.
- Keep the base and candidate evidence separate.

## 5. Setup

1. Record the candidate commit and executable SHA-256.

   Result: Each item of evidence identifies the candidate.

## 6. Tasks

### 6.1 UNIT — Input and capture tests

1. Run the wheel tests:

   ```sh
   cargo test -p horizon-ui app::canvas_scroll
   ```

   Result: Wheel input and smooth motion tests pass with the new event source field.

2. Run the device input tests:

   ```sh
   cargo test -p horizon-ui device_widget::tests::interact
   ```

   Result: Clicking outside the image releases held keys and ends keyboard capture.

5. Run the full local matrix from `AGENTS.md`.

   Result: The blocking and strict tiers pass. Record advisory findings separately.

### 6.2 DESKTOP — Base and candidate comparison

1. Start the base with the isolated desktop fixture.

   Result: The fixture shows synthetic terminals in a live view.

2. Exercise terminal input, wheel input, resize, and the menu search field.

   Result: The controls work. Record screenshots after launch and resize.

3. Repeat the same tasks with the candidate and the same fixture size.

   Result: The candidate preserves the controls and the left-aligned search text.

4. Restart the candidate with the same disposable session.

   Result: Panel positions and the selected workspace remain correct.

## 7. Pass criteria

- All applicable tests pass.
- The base and candidate show no unexplained visual difference.
- Record each unavailable platform separately.

## 8. Cleanup

1. Close each candidate through its normal close control.

   Result: The candidate exits. Other Horizon processes remain active.

2. Stop only the fixtures that this run started.

   Result: The task-owned displays and servers stop.

## 9. Record of results

Keep private evidence outside the repository. Record the candidate, commands, results,
and unavailable tasks in the pull request or the run report.
