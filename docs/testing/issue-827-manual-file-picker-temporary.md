# Issue 827 manual file selection qualification

Candidate: isolated `issue-827-manual-file-picker` worktree based on fresh main.
Use only a loopback synthetic page and disposable files. Observe the candidate
through an owned live native VNC Device panel and record its exact binary hash.

## Approved implementation boundary

- Host-owned chooser state with request identity, single/multiple mode, original
  target binding, cancellation, navigation/session invalidation, and errors.
- Chromium native chooser interception and Firefox native chooser event handling.
- A Horizon file-selection dialog, with filesystem work off the render thread,
  focus containment, cancellation, and selection returned to the original input.
- Equivalent existing agent attachment operations remain on `set_files`; expose
  chooser capability/status through shared browser metadata where applicable.
- Check Safari behavior and present an actionable unsupported explanation if its
  driver cannot provide a manual chooser. Remote manual steering and iOS are not
  expanded by this issue.
- Keep protocol/backend glue and UI rendering in focused modules. Expect roughly
  15–20 source/test files for the connected feature and its regression coverage;
  user explicitly approved this connected scope for #827.

## Baseline

1. Freeze a build from the unmodified base and record commit, hash and child PID.
2. Start a task-owned display, window manager, private state and loopback server.
3. Create/reveal its native Device viewer and verify three timestamped advancing
   observations with connected, received and displayed images.
4. Navigate through public browser MCP to a fixture containing a visible file
   input, a label associated with a hidden input, and a button calling input.click.
5. Click each upload trigger through public browser actions; inspect the host
   desktop for a usable chooser and the page for names, sizes and change events.

## Candidate functional lanes

- Visible single-file input: choose a synthetic text file; assert filename, size,
  one change event and a later read of the exact bytes.
- Hidden multiple input via label: choose two files, including an empty file;
  inspect readback, replace with one file, and verify previous files are removed.
- Programmatic trigger from a trusted button and keyboard activation.
- Cancel initial selection and cancel replacement; preserve selected files and
  change count. Escape must dismiss only the host chooser.
- Respect single/multiple selection and accept filters; invalid hints must not
  incorrectly reject files. Report unreadable/deleted files without losing focus.
- Keep target identity stable if the page replaces or removes the input while
  the dialog is open. Navigation, backend switch and shutdown invalidate requests.
- Start a human handoff with a pending chooser; select via the native host dialog,
  then use Done to return ownership and verify normal public MCP control resumes.
- Repeat open/cancel/select and ensure stale picker results cannot affect a new
  request or another browser panel.
- Verify Chromium and Firefox; check Safari support/fallback explicitly.
- Regression-check the previously delivered `set_files` contract.
- Change handlers may clear or remove the input after reading the files; one
  successful attachment must close the dialog without retrying that upload.
- Cover an iframe input and iframe navigation while its chooser is pending.
- Replace a pending request while its original probe is still completing; the
  newer request must take precedence without attaching files to the older input.

## Visual and lifecycle lanes

Capture launch, chooser, confirmed selection and resized/fit screenshots. Record
the interaction directly from the owned desktop and inspect decoded frames.
Check keyboard focus, narrow/wide layouts and no repeated filesystem scans during
idle frames. Verify live viewer presentation throughout changing output.
Close only the exact candidate normally, close the owned viewer, and verify
fixture children exit and the target expires. Delete this temporary plan after
qualification; retain private evidence outside the repository.

## Delivery

Run the full repository validation matrix in the final worktree, independent local
review, then create a ready PR. Request Copilot with timeline-event proof; resolve
in-scope findings and repeat validation on each changed head. Squash merge only
after current-head review, thread-aware audit, all applicable CI and smoke pass.
Verify merged patch and post-merge CI before removing the clean task worktree.
