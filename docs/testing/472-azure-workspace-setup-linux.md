# Linux Azure CPU workspace setup smoke

Temporary validation plan; remove after the applicable native pass. Nothing in
this plan authorizes Azure credentials, allocation, deletion or image publication.
The final candidate, dependencies and helper hashes must be reviewed and bound
before any build or runtime. Preserve existing sessions, workers and evidence.

## Outcome and prerequisites

- Extend the existing New/Review/Confirm/Check flow to explicit Azure CPU profiles.
- No profile editor, default provider, Git/PAT delivery, task start, attachment,
  Stop, compute Start, repair, automatic retry or cleanup is added here.
- Complete the combined LocalDocker/RunPod New UI and configured CPU prerequisites
  first; refresh onto their actual merged commits before final validation.
- Use the exact debug candidate on a new task-owned Xvfb/WM with PID-scoped input,
  isolated persistent owning session and `umask 077`. No pre-existing display,
  Horizon/Pi process, environment credentials or user config may be changed.
- Reuse existing reviewed native supervision/observation patterns; do not duplicate
  the #547 Azure acceptance campaign or the #551 Stop harness.
- For local no-cloud lanes, strip all provider credentials from the child environment
  and block only that owned child's external network. Use synthetic subscription,
  identity and registry values. No real `az` credential lookup is allowed.
- Record expected local session/config files separately from cloud-store/key files;
  preview/cancel must not create the latter. Snapshot hashes before and after.

## Required deterministic tests before launch

- All previous LocalDocker/RunPod parsing, ownership, single-flight, timing,
  recovery and unknown-outcome tests remain enabled.
- Azure has no selected default, image or program. Empty optional ceiling means
  no ceiling; a positive integer is passed as billing-currency micro-units unchanged.
- Zero, negative, fractional and overflowing limits fail with fixed diagnostics.
  RunPod retains its US-cents conversion; switching providers cannot leak limits.
- Preview and consent include all eight frozen profile fields plus exact image,
  disk size, repository, commit, branch, saved command/argv and owning session.
- Any profile field change invalidates confirmation before dispatch. Cancel,
  owner/home changes and a late result cannot restore stale consent.
- Azure confirmation is consumed once. An actual pre-storage refusal is not
  uncertain billing; a post-start `SetupUnconfirmed` or lost reply remains unknown,
  including when the view was closed. Original coordinates are never replaced.
- Actual noncreating Check of synthetic interrupted/binding-drift records preserves
  the allocation and binding, creates no key, and never reaches credential/provider
  work. Do not present these synthetic saved records as UI-created cloud resources.
- Deterministic rendering covers 89/90, 179/180 and 299/300 seconds. Initial Azure
  Submit uses the 300-second decision message; other submissions and noncreating
  preview/Check use 180. None of these messages cancels work or caps billing.

## Native local no-cloud lane

1. Launch the bound candidate normally in the isolated persistent session. Open
   Remote environments and New. Confirm no profile is selected and no image or
   command is filled. Capture baseline screenshot and filesystem observation.
2. Select an explicitly configured synthetic Azure profile. Enter a digest-pinned
   image on its registry, data-disk size, exact Git commit/work branch and literal
   saved Shell arguments. HPS fields must not be presented as Azure storage inputs.
3. Try empty, valid and malformed optional limits. Review must show the exact raw
   billing-currency micro-unit amount, or Not set, never infer USD or a live quote.
4. Review without checking consent. Verify all profile fields including the full
   managed pull-identity resource ID, subscription and declared price are readable;
   the Create control is disabled and nothing has been created.
5. Edit/cancel, close/reopen the overview, and change the owning session/config through
   normal isolated app paths. Verify old confirmation is discarded, not submitted.
   Do not change the real user's config or inject private application state.
6. Exercise keyboard traversal, the supported 800x600 minimum, wide window, resize
   and Fit. A 390px synthetic render may supplement these native checks; do not
   change or bypass the application's native minimum to obtain it.
   Inspect screenshots for long identity/image/argv wrapping and reachable consent,
   Edit/Cancel controls. Capture a bounded motion trace as well as still images.
7. Record matched idle and pointer-only traces before/after opening the form and
   review. No polling/network/file scans occur simply because the form is visible;
   only an actual pending request schedules the existing bounded repaint cadence.
8. Close the exact owned GUI normally and verify exit. Confirm no cloud store/key,
   provider request or new resource was created in this preview/cancel lane. Clean
   only task-owned display/session fixtures under their original cleanup guards.

## Positive creation evidence and reporting

Successful provider dispatch, binding-before-key ordering and recovery are covered
by the configured CPU API's injected-backend tests. These are not live Azure proof.
If real Azure UI creation is required, extend the existing authorized #547 campaign
only after a separate exact profile/image/cost/credential/cleanup review: execute
actual UI Confirm once, retain its original locator, manually Check without ensure,
and independently verify the same task-free worker/binding and no task launch.
Never replay uncertain Confirm or borrow an older head's cloud result as UI evidence.

Report exact head/tree/binary, test and screenshot/trace paths, no-I/O observations,
normal-close/cleanup proof, and explicit live-cloud exclusions or separately bound
results. Build success and synthetic tests alone are not a native visual pass.
