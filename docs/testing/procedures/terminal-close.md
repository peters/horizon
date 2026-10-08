---
procedure: terminal-close
feature: Terminal close and PTY cleanup
platforms: [linux]
cost: none
destructive: yes
secrets: none
owner: peters
---

# Terminal close test procedure

## 1. Purpose

This procedure tests terminal destruction after event loop exit.
It separates caller response, native UI response and child process cleanup.

## 2. Applicability

The unit fixture requires a Unix shell and SIGHUP.
The native smoke requires Linux, X11 and the public `device_panel` tool.
The native smoke does not force the event loop handle to finish before terminal destruction.
The deterministic unit fixture tests that condition.

## 3. Safety

> **CAUTION:** CLOSE ONLY RESOURCES THAT THIS RUN OWNS.
> A shared terminal can contain work from another person.

Use a private fixture with synthetic output and no credentials.
Record each owned process ID, start time and executable identity.
Never stop a process after its identity changes.

## 4. Equipment and preconditions

- A frozen candidate executable, its SHA-256 and source identity.
- The isolated desktop launcher from `scripts/device-smoke/`.
- The scoped `horizon-device` CLI and explicit target file.
- A synthetic child that ignores SIGHUP and waits for a private release file.
- A second terminal that prints a counter once each second.

## 5. Setup

1. Copy the desktop launcher into a new private task directory.

   Result: The repository launcher and shared user files stay unchanged.

2. Configure the two synthetic terminals in the private launcher copy.

   Result: One child waits for release. The other child prints a counter.

3. Start the isolated candidate with the private launcher.

   Result: The launcher creates an explicit native target and a loopback VNC endpoint.

4. Record the candidate and owned process identities.

   Result: Evidence identifies the exact executable and processes under test.

## 6. Tasks

### 6.1 TERM-CLOSE-UNIT — Finished event loop

1. Run the focused regression in the candidate checkout.

   ```bash
   cargo test -p horizon-core terminal::lifecycle::tests::dropping_a_finished_event_loop_does_not_wait_for_its_live_child -- --exact
   ```

   Result: Terminal destruction returns before child release. Background PTY cleanup completes after release.

The fixture opens its release gate before it reports a failure.
For an old-code comparison, retain the same fixture and change only the terminal destruction implementation.

### 6.2 TERM-CLOSE-LIVE — Native response

1. Create an owned Device viewer for the declared loopback endpoint.

   Result: The viewer connects to the isolated candidate.

2. Reveal the viewer for its first presentation.

   Result: Public inspection reports actual displayed pixels.

3. Save three inspections at least two seconds apart while the counter changes.

   Result: Display and transport evidence identify advancing candidate activity.

4. Start the viewer video before input.

   Result: The recording contains the panel close operation.

   > **CAUTION:** CLOSE ONLY THE GATED TERMINAL THAT THIS SYNTHETIC FIXTURE OWNS.
   > This operation stops its event loop and starts child cleanup.

5. Close the gated terminal panel through normal candidate UI input.

   Result: The panel disappears. The counter continues to advance.

6. Examine the gated child identity before release.

   Result: The exact owned child stays alive while the candidate responds.

7. Click Fit in the candidate.

   Result: The candidate responds while the child stays alive.

If the person moves away from the viewer, continue without another reveal.
Record prior display proof and later transport evidence separately.

### 6.3 TERM-CLOSE-CLEANUP — Owned child release

1. Create the private release file.

   Result: The exact child exits and background cleanup reaps it.

2. Examine the candidate and counter process identities.

   Result: Both processes stay alive after child cleanup.

3. Resize the candidate window through the scoped native CLI.

   Result: The candidate renders the changed window size.

4. Click Fit in the candidate.

   Result: The remaining terminal fits the canvas.

5. Stop the viewer video.

   Result: Final status reports no encoder failure and a positive frame count.

6. Copy the finalized recording into private evidence.

   Result: The recording remains available after viewer closure.

## 7. Pass criteria

All three task IDs pass.
The unit fixture proves the completed-handle condition.
The native smoke proves responsive panel close and separate owned child cleanup.
One Linux smoke does not establish Windows or macOS runtime results.

## 8. Cleanup

> **CAUTION:** CLOSE ONLY THIS RUN'S EXACT CANDIDATE WINDOW.
> The launcher removes its private configuration and home after window closure.

1. Close the exact candidate window through normal UI input.

   Result: The launcher stops its owned desktop processes and revokes its target.

2. Examine the recorded identities and private runtime directory.

   Result: No owned process remains. The private configuration and home are removed.

3. Close the owned Device viewer.

   Result: Its connection closes without changes to other viewers.

## 9. Record results

Use the task IDs in a report under `docs/testing/reports/`.
Record source identity, executable hash, results, limits and cleanup proof.
Keep local paths, process IDs and private recordings out of public reports.
