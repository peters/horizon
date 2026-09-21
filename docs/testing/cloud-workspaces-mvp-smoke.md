# Cloud Workspaces regression smoke guide

This is a permanent acceptance and regression guide. Keep it with the feature;
update scenarios when behavior changes. Store run-specific identities, logs and
original evidence in a durable private directory outside temporary storage. A
completed previous run does not substitute for validation of a changed candidate.

Read [setup and behavior](../cloud-workspaces.md), the
[worker image contract](../../examples/cloud-worker/README.md), and the
[isolated native fixture](../../scripts/device-smoke/README.md) first.
The [delivery record](../plans/cloud-workspaces-mvp.md) distinguishes implemented,
locally verified, cloud verified and user-accepted behavior.

## Run contract and prerequisites

Record the source commit, dirty diff if any, platform, tool versions, tested image
digests, scenario IDs, operator and UTC start. Use synthetic repositories and
websites. For the Horizon issue and PR, publish Horizon screenshots only; other
application screenshots remain private, including those embedded in its panels. Never put secrets in repository YAML, command arguments, screenshots or
reports. Private settings bind absolute credential-file paths; use restrictive
permissions and expiring repository-scoped registry/Git credentials. Record only
credential references and expiration, never their values.

Before a real provider test, obtain allocation and spend authorization, record
quoted CPU/GPU prices and a bounded cleanup deadline, and save a read-only inventory
of pre-existing workers. Maintain a separate ledger of task-created operation IDs
and worker IDs. A name prefix alone is insufficient authority to delete a worker.
Never delete unrelated resources or remove an uncertain operation journal to retry.

Prerequisites include the Rust/system dependencies in `AGENTS.md`, Docker with
BuildKit/buildx, Git/LFS, SSH, a registry, a provider account, and credentials for
agents being tested. Native Linux testing additionally needs Xvfb, Openbox,
x11vnc, bubblewrap and DBus. Recording needs ffmpeg and a display-scoped recorder.
Use an actual platform machine for macOS/Windows graphics/input claims.

Choose a durable `evidence_dir` outside the checkout and `/tmp`. Create a new
subdirectory per candidate and scenario. Suggested contents:

```text
candidate.json             # commit, hashes, OS, tool versions, actual process
inventory-before.json      # private provider baseline and task resource ledger
validation/                # commands, complete logs, exit codes and timings
scenarios/<id>/             # assertions, screenshots, recordings and decoded frames
original-design/           # unchanged approved references and checksums
publication/               # reviewed redacted copies, never originals
report.md                  # pass/fail/blocked/skipped with evidence references
SHA256SUMS
```

## Build and required local matrix

Use an isolated worktree and its own Cargo target directory. Run from the exact
checkout to be published. Capture each command and exit code separately:

```sh
cargo fmt --all -- --check
./scripts/check-maintainability.sh
python3 -m unittest discover -s examples/cloud-worker -p 'test_*.py'
RUSTFLAGS='-D warnings' cargo test --workspace
RUSTFLAGS='-D warnings' cargo test --workspace --features speech
cargo clippy --all-targets --features speech,trace-profiling -- -D warnings
cargo clippy --workspace --lib --bins --examples --features speech -- -D warnings -D clippy::unwrap_used -D clippy::expect_used
cargo clippy --workspace --all-targets --features speech -- -D warnings -W clippy::pedantic
cargo build -p horizon-ui --bin horizon --features cloud-panel-mock
cargo build -p horizon-device --features cli
cargo build -p horizon-core --example cloud_deploy
```

Pedantic is advisory under current repository policy; retain and triage its
findings rather than calling a failed lane green. If a timing-sensitive test fails,
retain the first log, investigate, and record any serialized complete-suite retry
with `RUST_TEST_THREADS=1`. Report the retry separately. A selective passing test
is not proof that the complete suite passed.

Freeze the executables before launch in the candidate evidence directory. Hash
those copies. Do not overwrite a running candidate with a rebuilt binary.
For design fixtures the `cloud-panel-mock` feature is required; these fixtures
must remain visibly labelled. Only the operational provider may allocate workers.

Building the feature does not activate fixtures. Follow the separate
[fixture launch instructions](../prototypes/cloud-panels.md): use an empty private
configuration and set `HORIZON_CLOUD_MOCK_DIR` to a fresh private fixture directory
in the isolated application's environment. `HORIZON_CLOUD_MOCK_VNC` optionally
supplies an explicitly owned loopback viewer endpoint. Keep the fixture data and
configuration separate from real-cloud state. Explicitly unset both mock variables
for operational cloud and persistence tests.

## Native viewer and capture setup

While a cloud or one of its child panels is fullscreen, open the command palette
and hold Escape. Only the palette should close; repeats and key release must not
leave fullscreen or reach the terminal. A fresh Escape then returns exactly one
level. Repeat with an outside-click palette dismissal, which must not swallow a
later navigation key.

1. Start `python3 scripts/device-smoke/serve.py --horizon <frozen-binary> --native-view
   --state <new-private-state-directory>`. The documented baseline uses a
   1600×1000 desktop. For 4K acceptance, preserve a task-specific launcher using
   3840×2160 while retaining atomic display allocation, private home/config,
   namespace isolation, loopback VNC and exact process ownership. Save that launcher
   with the run evidence; do not modify an active user's desktop or display.
2. Read `vnc_address` from the fixture's manifest. Use public `device_panel`
   `list`, then `create` for this endpoint in the calling agent's current workspace.
   Inspect the returned panel: require connected, image received, image displayed,
   and advancing frame sequence during changing output. `visible: true` alone is
   insufficient. Pause interactive testing if the live native image goes off-screen.
3. Find the actual Horizon child PID within the fixture's owned process tree.
   Record `/proc/<pid>/exe` and its SHA-256 against the frozen binary. The launcher
   PID may belong to bubblewrap. Match the exact window to that child PID.
4. Drive native controls through the fixture's explicit device target. If native
   window-manager input is needed, scope it to that display, PID and window. Use
   fresh geometry after resize. Browser page interactions use only public
   `browser_*` MCP tools; never use native input to drive a browser page.
5. Record directly from the isolated display before the flow begins. The Device
   panel is a read-only viewer and does not record video. With a compatible X11
   recorder, a bounded example is:

   ```sh
   ffmpeg -f x11grab -video_size 3840x2160 -framerate 10 \
     -i "$test_display" -t 120 -c:v libx264 -preset veryfast -crf 23 \
     -pix_fmt yuv420p "$evidence_dir/scenario.mp4"
   ```

   Set `test_display` from the owned fixture manifest. Verify frames advance early;
   a live recorder PID is not proof. On Xvfb configurations where x11grab stalls,
   use a saved display-scoped ImageGrab/raw-frame recorder and record that choice.
   Do not substitute screenshots for motion evidence.
6. Capture launch and post-resize screenshots using the configured device CLI:

   ```sh
   "$device_bin" --target "$device_target" screenshot "$evidence_dir/launch.png"
   ffprobe -v error -select_streams v:0 -show_entries stream=width,height,nb_frames \
     -show_entries format=duration -of json "$evidence_dir/scenario.mp4"
   ffmpeg -v error -i "$evidence_dir/scenario.mp4" -f null -
   ffmpeg -v error -ss 20 -i "$evidence_dir/scenario.mp4" \
     -frames:v 1 "$evidence_dir/frame20.png"
   ```

   Inspect decoded frames before, during and after movement. Require 3840×2160 for
   native 4K evidence. Retain original capture hashes and candidate references.

### Persistent cloud launcher for restart scenarios

The baseline `serve.py` passes `--ephemeral` and removes its private data on exit.
It is suitable for baseline viewer/input checks only. Do not use it unchanged for
U09 or R01–R09. Save a task-owned copy beside `sandbox.py` in the private run
directory and make these explicit changes before launching it:

- Use a 3840×2160 Xvfb screen and a suitable initial client window. Retain atomic
  `-displayfd`, private namespace/DBus and loopback native VNC startup.
- Replace the heartbeat workspace with an empty synthetic workspace. Create the
  synthetic repository under `box.data`; all local repository bindings must point
  to its path inside the namespace. Keep the configuration at
  `box.data / 'horizon.yaml'`, pass that exact path on every launch, and omit
  `--ephemeral` so normal runtime persistence is enabled.
- Install private cloud settings at the sandbox's private home `.horizon/cloud/`
  path. `sandbox.prepare` returns `sandbox_env` and `data`; use those values rather
  than the developer's real home. Copy only task credential files into private
  fixture data or explicitly bind their paths read-only. Bind the authorized
  Docker socket if needed and point `docker_host` to that socket. Never inherit
  arbitrary host agent login state. Keep evidence outside `data`.
- In the existing `application_done` branch, support an operator-created
  `restart-request` file under the state directory. Remove the completed
  application from `children`, consume the marker and call the same `spawn` helper
  with `[str(app), '--config', str(config)]`. Keep Xvfb, DBus, VNC, namespace,
  environment and data alive. Without the marker, use normal fixture shutdown.
- Before closing the exact test window normally, create the marker with
  `touch "$fixture_state/restart-request"`. This is the restart command for the
  adapted launcher; it is **not** a flag of the checked-in baseline. Then close
  the application using its normal window-manager close path. Record the new
  actual child PID/hash after relaunch. Do not signal the developer's application.
- Preserve private `data` after fixture shutdown until evidence and credential
  cleanup are complete; retain endpoint expiration and task-owned child cleanup.
  Archive the exact adapted launcher, its diff against the baseline and its
  sandbox companion in the private evidence manifest before the first run.

This deliberate adaptation is currently required; there is no installed cloud
smoke launcher with these flags. A future reusable harness must retain the same
isolation and ownership assertions. The guide does not claim the baseline fixture
alone proves cloud restart persistence.

## Synthetic source and worker fixtures

Create a local repository with a committed README, a small Python module and
unittest suite, `.horizon/cloud.yml`, `.dockerignore`, a build marker, a Git LFS
asset and a pinned local submodule. Make a second commit so revision selection can
be distinguished from HEAD. Add an uncommitted sentinel and an ignored file. Select
the first committed revision and verify its exact hash remotely; neither sentinel
may appear. Include a branch name containing a shell metacharacter in a local
validation case and confirm it remains data, never shell syntax.

Prepare CPU and GPU images from the example worker contract. Pin base images and
record the final remotely resolved digests. Use separate build and image-only
profiles. The image must include SSH, tmux, Git and the required agent/browser/
desktop services; an arbitrary application image is not sufficient. Keep private
source and credentials out of image layers and build contexts. Test Git credential
support on an image containing that optional contract before worker allocation.

For failure tests use separate state roots and operation identities. Use a local
HTTP provider simulator for deterministic timeout/uncertainty tests; retain real
provider tests as a separate lane. Never describe simulated faults as live faults.

## Configuration and deployment scenarios

| ID | Action | Required outcome |
|---|---|---|
| C01 | Load valid defaults, explicit profile, CPU and GPU profiles | Central defaults and selected resource requirements match the typed specification |
| C02 | Invalid version/provider/default/profile, unsafe paths, missing local objects or credentials | Clear validation error before allocation; no secrets in Debug/errors/progress |
| C03 | Two clouds/profiles sharing or separating credential references | Explicit bindings determine sharing; names alone do not imply isolation |
| D01 | First local Dockerfile build and push | Visible ordered progress and expandable output; remote digest available before first allocation |
| D02 | Repeat unchanged build, then change a committed build input | Cached layers reused where applicable; changed content produces the expected new digest |
| D03 | Image-only profile | No Dockerfile build; immutable digest and worker contract verified |
| D04 | Bad Dockerfile, denied push and registry outage | Retryable card, no allocated worker; retry preserves intended source/profile |
| D05 | Cancel build and push using bounded test children | Children terminate and state is retryable; no allocation; label mocked versus live cancellation |
| D06 | Expire credentials used by local Docker image preparation | Initial pull succeeds; subsequent preparation fails without allocation; no token in logs |
| D07 | Invalid worker contract or missing optional Git helper | Reject before allocation, including a saved Prepared specification and late Git opt-in |
| D08 | Real bounded CPU deployment | Correct immutable image/resources, SSH readiness, source and runtime contract |
| D09 | Real bounded GPU deployment | Correct GPU profile; `nvidia-smi` reports expected device; source and agents work |
| D10 | Unavailable capacity / provider rejection | Typed error and explicit retry, no misleading Ready state |
| D11 | Delayed create response beyond client timeout | Persist uncertainty; reconcile same operation; count exactly one create request |
| D12 | Restart during an uncertain create | Same operation survives; no blind second allocation or cleared safety fence |
| D13 | Expire only the provider pull binding, or cause pull/boot/readiness failure after accepted creation | Retain known worker reference for inspection and explicit cleanup; do not silently allocate another |

For each real allocation record identity, digest, quote and start/end time before
cleanup. Verify remote image, Git revision and service readiness, not just an API
response saying RUNNING. Save resource inventory after each failure/retry.

## Cloud UI and membership scenarios

| ID | Action | Required outcome |
|---|---|---|
| U01 | Open baseline workspace, then create multiple named clouds | Existing local panels remain usable; title/repository/revision/profile flow is modal and clear |
| U02 | Cancel creation; use invalid fields; reopen | No ghost worker/cloud and no input leaking into a terminal underneath the modal |
| U03 | Add multiple instances using ordinary panel picker inside each cloud | Correct remote environment and immutable cloud/workspace membership; real interactive panels |
| U04 | Default, Rows, Cols, Grid independently in three clouds | Only selected cloud changes; calculations and controls match workspace behavior |
| U05 | Add a panel while cloud is fullscreen in Default layout | New child is visible beside existing children without initial overlap |
| U06 | Attempt dragging children into another cloud/workspace | Membership remains unchanged, including after restart |
| U07 | Cloud fullscreen, focused child F11, Escape twice | Child returns to same cloud, then exact previous overview; session identity unchanged; navigation press/repeats/release never reach the terminal or cancel agent approvals; a fresh Escape in overview still reaches the terminal |
| U08 | Resize small, Fit, restore large, Fit; drag and resize panels | No clipped/overlapping controls, oscillation or snapping; inspect video as well as stills |
| U09 | Save independent layouts/views and normally restart the isolated client | Cloud IDs, membership, panel IDs, layouts and session references match |
| U10 | Labelled three-provider design fixtures | All scenarios retained; non-operational providers never presented as real deployments |

For approved composition use the same viewport, UI scale and window dimensions as
the references. The original MVP comparison used a 3600×1960 client window at scale
2 within a 3840×2160 desktop. Compare overview and every cloud fullscreen. Declare
masks for dynamic terminal/website/session content and intentional label changes.
Do not align or rescale screenshots to improve a score. Record tolerance, mask
coverage and mismatched regions; inspect them visually. A similarity percentage
does not establish functional correctness or user acceptance.

## Real agents, source isolation and remote tools

1. Add at least two instances of each authenticated agent type through ordinary
   panels. Use the actual first-use login flow or supported API-key binding.
   Never capture or publish login codes/keys. Mark unavailable authentication as
   skipped with a reason; do not substitute a shell or fake terminal.
2. In every instance run the committed baseline tests. Ask two agents concurrently
   to create different tiny modules and four-test suites in their own worktrees.
   Example: one implements vector addition, another mean calculation. Require test
   output and inspect the actual files, branches and worktree paths afterward.
3. Verify distinct branch/worktree and stable tmux identity per agent. Ensure one
   agent's uncommitted files are absent from the other's worktree. Combining changes
   must remain explicit; do not claim worktrees eliminate merge conflicts.
4. Have an agent use public browser MCP to read Example Domain. Add/view the same
   cloud browser panel and compare its controller label with the actual tool owner.
   Transfer control to a second agent and verify the label updates.
5. Have an agent use worker-local device doctor/screenshot and a bounded harmless
   desktop action. Add the cloud's native desktop viewer. Check read-only viewing
   versus the actual last-input agent; clicking the viewer must not inject input.
6. Repeat browser/device actions from another cloud and confirm ownership/control
   remain scoped to that cloud. Do not use a laptop-side browser or input service
   to satisfy a worker-independence claim.
7. Start a bounded remote test or tool wait, disconnect the client, and verify it
   completes on the worker. Reconnect and inspect its result. Record actual agent
   process IDs/start times and tmux pane IDs before and after, not only session names.

## Optional Git credentials and remote PR smoke

Use an explicitly authorized disposable repository and short-lived repository-only
token whenever available. Binding selects a repository; it does not reduce a broad
token's original account permissions. Never revoke the user's original account
login as part of task-copy cleanup.

1. Configure an explicit matching local-repository binding with private token-file
   path and author identity. Validate nonmatching and duplicate bindings. Deploy
   or reconnect, and check the worker credential directory/file modes are 0700/0600
   without printing contents. Check permission rejection happens before any write.
2. Run synthetic helper tests for matching/nonmatching HTTPS repository requests,
   absent credentials and removal. Confirm no token in argv, progress, logs,
   persisted deployment state, YAML, Git config or image history.
3. After the final implementation commit, ask an actual remote agent to create one
   harmless file, run its test, commit on a unique task branch, push, and open a PR
   in the disposable repository. Respect local branch/commit/PR naming policy.
   Record model/effort, actual agent session and resulting branch/head/PR URL privately.
4. Independently inspect the PR and its exact one-file diff. Ask the same agent to
   close it unmerged and delete only its branch. Independently verify CLOSED,
   absent merge commit and absent test branch. A host-side PR creation does not
   satisfy this scenario.
5. Remove the binding, then successfully reconnect the same worker. Verify its
   credential copy is absent and fresh authenticated CLI access is unavailable.
   Existing source/worktrees remain intact. Removal during an offline worker is
   pending until reconnect succeeds; do not claim immediate revocation.
6. If the provider prevents nested user namespaces, record the actual agent error
   and obtain/use the authorized command approval for that disposable worker.
   Do not silently relabel an approval/sandbox failure as a product success.

## Disconnect, restart and failure recovery

| ID | Action | Required outcome |
|---|---|---|
| R01 | Close only the isolated client normally; leave worker running | Actual agent/tool processes continue and bounded work completes |
| R02 | Restart same frozen candidate and private state; reconnect | Same worker, worktrees, tmux panes and process identities; tunnels/presentation restored |
| R03 | Interrupt only a task-owned SSH/tunnel connection | Honest disconnected state; explicit reconnect restores same remote sessions |
| R04 | Stop an identity-verified task worker, then Resume | Same provider worker when capacity permits; stopped processes may be lost and are reported honestly |
| R05 | Provider rejects Resume | Retain stopped/failed state and original worker reference; record real error without claiming recovery |
| R06 | Remove a disposable worker externally, then reconnect | WorkerLost, no automatic replacement allocation |
| R07 | End one disposable remote agent process, then reconnect | Lost process is reported; no silent fresh session under the old identity |
| R08 | Restart during build/push/readiness | Reconcile persisted phase safely; no compute before remote image availability |
| R09 | Restore ordinary local workspace and cloud state | No migration damage, phantom memberships or automatic stopped-viewer connections |
| R10 | Open the sidebar context menu for a workspace containing clouds; attempt direct detachment | Open in New Window is disabled with an explanation; the central action also refuses detachment |
| R11 | Restore an older state containing a detached cloud workspace and an ordinary detached workspace | Cloud and all members restore together in the main window; ordinary workspace remains detached |
| R12 | Select an ordinary detached workspace and attempt cloud creation | Clear instruction to move it to the main window; no cloud, worker or sessions allocated; creation succeeds after reattachment |
| R13 | Move a workspace to positive and negative coordinates, then create two clouds | Correct 48-pixel overview gap; first creation immediately fits the actual cloud; no drift after reconcile/restart |
| R14 | Try sidebar workspace presets with Default and named cloud layouts; add and resize an ordinary panel | Parent presets disabled, cloud geometry and layouts preserved, ordinary workspace presets still work |

Preserve both before/after process records, operation journals and fresh provider
inventories. Do not destroy the retained user-acceptance worker to exercise failures;
allocate a separately recorded disposable case within the authorized budget.

## Cleanup and repeatability

- Close the exact test window normally; preserve the user's Horizon, terminals,
  desktop and unrelated sessions. Close only task-owned Device panels when finished.
- Delete task workers only after matching their saved identities and required
  evidence. Verify absence using a fresh provider inventory and compare protected
  pre-existing resources. Stop alone may retain billable storage.
- Remove task credential copies and revoke task-scoped registry credentials after
  testing. Verify denied access without logging secrets. Report any retained worker,
  quote, purpose, deadline and pending cleanup explicitly.
- If retaining a live environment for user acceptance, record that state separately
  from cleanup completion. Preserve generic worktree results and private evidence.
- Check fixture children exit and target expires when the fixture is retired.
  Keep original images/videos, checksums, selected image digests and example YAML in
  durable storage; remove disposable build/auth state according to the test ledger.
- Repeat affected lanes after behavior changes and rerun repository-required gates
  for each final pushed head. New source invalidates older behavior evidence until
  the affected smoke is repeated; documentation-only changes should be labelled.

## Publication and report template

Review every attachment as public. Publish only synthetic fixtures and curated
copies. Redact authentication material and private repository/resource/host/session
identifiers; document redactions and video cuts. Retain unchanged originals and
checksums privately. Verify uploaded attachments and downloadable YAML are accessible.
Never claim a video shows a step performed outside its recording.

```text
Candidate: source SHA / binary SHA256 / image digests / platform
Native viewer: connected + received + displayed + advancing frames / timestamp
Scenario: ID and description
Result: pass | fail | blocked | skipped
Evidence: durable files + checksums + exact candidate
Assertions: observed behavior, before/after identities, test counts
Limitations: simulated versus real faults; authentication/platform omissions
Resources: allocated / deleted / retained; quote and cleanup deadline
Credentials: task bindings removed/revoked or explicitly pending
Gates: implemented / local verified / cloud verified / user accepted
Follow-up: concrete defect or future feature; owner and retest lane
```

An independent reviewer should be able to reproduce the run from this guide plus
its private run manifest without relying on a prior conversation. Keep regressions
and their decisive scenarios in this guide after the initial MVP is accepted.


### Deployment progress and estimates

- Run a fresh image build and push with a generic synthetic layer large enough to observe several seconds of transfer. Record the exact candidate, immutable digest and native video before deployment starts.
- Every active phase shows an advancing elapsed time. Completed phases retain their observed duration. A failed or cancelled phase freezes its timer; reconnect begins a new explicitly observed attempt rather than claiming historical timing.
- Build output reports completed/discovered steps, including cache hits. Do not treat the currently discovered step count as a known final build total.
- Push shows layer completion, Docker-reported bytes, recent throughput and estimated remaining time once all remaining layer sizes are known. Cached or mounted layers must not inflate transferred bytes or speed. Queued layers with unknown size keep the ETA unavailable.
- Repeat with an image-only profile and a cold local pull. Download counters advance; extraction does not count those bytes again. Cancellation stops the task-owned transfer process without allocating a worker.
- Transfer a committed repository with LFS assets. Verify asset-check progress and source-upload counters, then verify the remote revision and object integrity.
- Delay UI event consumption while transfer events accumulate. Rates and completed durations must use producer timestamps, not the time a queued event is rendered. A stalled transfer must age out an earlier fast rate; retries must reset the rate baseline.
- Capture the runtime card at native 4K during measurable transfer, after cancellation/failure, and after resize/fit. Ensure elapsed times, bars and ETA text fit the approved card width without overlapping lifecycle controls.

## Capability-selected clean image regression

Retain this matrix for future tests. Use exact candidate helper hashes, immutable
image digests, private fixture state and a visible native Device panel.

| Profile | Expected optional executables/tools |
|---|---|
| Legacy YAML without capabilities | Original three agents, Chromium, desktop |
| Explicit `{}` | None; SSH/Git/tmux/source/shell only |
| Each single agent | Only that agent, no browser/device tool registrations |
| Native pair | Selected two agents + desktop/device, no browsers or third agent |
| Chromium only | Chromium and browser tools, no Firefox or device tools |
| Firefox only | Firefox + geckodriver, default browser is Firefox |
| Both browsers | Explicit engine respected; state retained through reconnect |
| Full | All requested agents/engines/desktop work together |

1. Audit OCI compressed manifest sizes and history before rebuilding. Record old
   ancestry bytes, helper sizes and individual agent install layers. Build from a
   clean compatible OS/workload base with stripped final binaries.
2. Test each profile's image contract with networking disabled. Request a missing
   agent, browser, driver and desktop helper; verify failure before provider POST.
   Repeat using a saved Prepared deployment specification and after cancellation.
3. Inspect final executable inventory, MCP configuration and active processes.
   Unselected software must be absent in the minimal recipe. Re-run configuration
   against a previously full HOME and verify managed disabled entries are removed
   while unrelated settings remain. No credential may enter context/cache/layers.
4. Attempt disabled panel/session creation through every supported interface.
   Verify a clear unavailable reason and no session directory, launch fence,
   worktree, browser marker or process was created.
5. On the native profile, two real agents edit separate worktrees, use device input
   and show real ownership without any browser installed. Disconnect the laptop;
   commands, tools and tmux must remain alive. Reconnect to the same worker/PIDs.
6. On the browser profile, create Firefox and Chromium through public browser MCP,
   confirm actual engine state and advancing frames, then disconnect/reconnect.
   Request a disabled engine and verify rejection without fallback.
7. Measure first local build and push, cached build and push, existing immutable
   image deployment to worker Ready, app readiness and first visible frame, each
   with its own timestamp. Record image pull/cache circumstances. Measure reconnect
   separately from delete/recreate. Worker Ready is not application-visible proof.
8. Run every repository's applicable tests; GPU workloads retain CUDA/TensorRT.
   Application captures are sent privately, never embedded in the Horizon issue/PR.

9. On a real provider container, test actual agent shell execution in addition to
   CLI version checks. Record namespace/sandbox restrictions and the supported
   permission mode used. A pending approval is not completed unattended work.
   Confirm both agents can call only their enabled MCP servers, preserve first-use
   login state across reconnect, and edit distinct worktrees.
10. Overflow the runtime card with long profile/image strings, errors, expanded
    logs and Stop/Delete confirmations. Its outer bounds stay fixed and scrolling
    reaches all controls. Select a second profile by pointer and keyboard in a
    600px-high creation dialog; Escape/reopen restores initial focus once.

11. Browser controller labels follow the live ownership lease. During a bounded
    public MCP wait, capture the controlling agent label; after the ten-second
    idle lease expires, confirm the UI reports no active agent. Do not treat an
    idle label as evidence that the preceding input was uncontrolled. Device
    labels distinguish active control from the last input actor.

### Restore ordering and slow source transfers

- Restore saved Firefox and Chromium panels with worker Ready preceding discovery. Save and restart again before discovery; each backend, URL and identity must survive. Discovery repairs an old mismatched view using the same worker browser, without starting a local browser or respawning agent sessions.
- Repeat with a lost worker browser. Show explicit process loss, retain the identity, and require explicit creation of a replacement.
- Transfer a committed multi-gigabyte LFS archive over a slow connection for more than ten minutes. Byte progress, rate and ETA must advance; integer percentage alone is insufficient. Cancel, reconcile the same worker, and retry. Check advancing KB while percentage is unchanged and advancing percentage while multi-GB amounts are rounded. File transfers fail after ten minutes with neither counter advancing or the six-hour total bound; image transfer limits remain unchanged.

### Cloud event delivery during child-panel fullscreen

Keep a native Device child fullscreen while one remote agent performs reversible
input. Verify the caption changes to that actual agent without leaving fullscreen.
Have a second agent perform input and verify the caption changes again; after
input completes, distinguish the last input actor from an active controller.
While a child remains fullscreen, finish or fail a background cloud deployment and
confirm its events are processed before returning to overview. Retain the same
fullscreen child and preserve membership. Repeat with nested cloud fullscreen and
after reconnect; a stale caption refreshed only by Escape is a failure.

## Opt-in hosted mobile browser regression

Use synthetic application data in public evidence. Keep real application
screenshots and all private account metadata outside this repository.

1. Omit `capabilities.browserstack`: deploy native/minimal profiles, verify no
   remote credential read, prompt, tunnel or advertised browser tool.
2. Declare two existing physical-device targets, no local browsers, and one
   worker-local HTTP port. With no local repository grant, verify failure before
   any provider allocation. Repeat with a grant for another checkout, an account
   outside the grant, missing credentials, malformed YAML and an older image.
3. Build the remote-only image. Verify browser tools and Local runtime work while
   local Chrome/Firefox and desktop executables are absent. Audit image history
   and context: no credential values, runtime files or private source.
4. Deploy using an authorized OS-store binding. Confirm source transfer and
   normal agent worktrees/sessions. Verify private runtime file permissions and
   that no secret appears in argv, build output, progress logs or saved state.
5. Through public browser MCP, read shared provider capacity, create the iPhone
   target, assert provider-reported physical identity, navigate to the worker's
   synthetic app and exercise semantic input. Capture a private screenshot.
   Close it and verify release before creating the Android target; repeat.
6. Repeat agent-driven creation through a CLI plan. Check actual target labels,
   fullscreen/restore, browser ownership and disabled local-engine behavior.
7. Disconnect/restart the Horizon client while the remote agent interacts with
   its phone panel. Reattach to the same worker, agent, tunnel and remote browser
   identities. No replacement session or worker may be allocated automatically.
8. Inject a definite remote-create rejection and an uncertain release using the
   integration fixture. Retain uncertain allocation records, refuse false close
   success and reconcile only the original allocation. Wrong targets on the same
   panel identity must be rejected. A worker-service crash must expose retained
   unknown allocation history and prevent silent fresh allocation.
9. Revoke remote access. Verify hosted-device release before removing the private
   key copies; verify the exact tunnel process exits while native agents stay
   alive. Without its machine-local grant, reconnect must refuse transfer. With
   the grant restored, reconnect may explicitly reinstall it.
10. Stop/Delete a remote-enabled cloud and prove hosted devices are released before
    compute disappears. Simulated release failure must block teardown and remain
    visible. Preserve unrelated provider sessions, workers and credentials.
11. Run focused configuration, credential, worker-image and UI tests, then the full
    validation matrix on the final candidate. Repeat the decisive live smoke after
    current-head review/CI; record immutable images, candidate hash, timestamps,
    cleanup receipts and private screenshot/email references in the local ledger.

Account-level mobile-device regression: declare only `browserstack.provider` and
explicit worker ports, grant the account for the local checkout, and verify an
agent can open a second configured device absent from optional preferred
`targets`. A different ungranted account must fail before compute allocation.
No profile may export credentials solely because repository YAML requests them.

### Provider catalog regression

- Declare the account and Local ports with no preconfigured device targets. An
  absent account grant fails before worker allocation; omitted capabilities never
  resolve or transfer account credentials.
- On the worker, call `browser_provider_devices` through public MCP and a CLI plan.
  Search phones, tablets and desktops, paginate, and select a combination that has
  never been configured as a Horizon target. Verify the CLI uses the same policy.
- Create from the returned reference; verify provider-confirmed device identity.
  A forged, stale or another account's reference must fail before allocation.
- Rebind the account and verify old discovery results cannot bypass the binding.
  A provider timeout, 401, unsupported provider, oversized query and cancelled
  discovery must leave no compute/device allocation or credential output.
- Disconnect Horizon while an agent controls the device. Reconnect to the same
  worker/session; close the device and verify exact release before revoking the
  copied credential and terminating the task worker.

### Worker container restart and private tunnel recovery

- Retain an owned hosted-device allocation, record worker/container identity, and
  restart only that task worker. Verify that vanished agent and browser processes
  are reported as lost; reconnect must never respawn them under their old identity.
- With an image that records its container incarnation, reconnect the same cloud.
  The private tunnel may restart only after matching shutdown evidence or a proven
  container change. A legacy, malformed or same-container record without matching
  shutdown evidence remains fenced.
- Fail reconnect before Ready. The explicit remote-device release action must
  remain available, complete through its own response channel, and preserve the
  setup failure. Pending release blocks concurrent lifecycle actions.
- After a failed release, confirm the retained allocation state before retrying.
  Successful release or Stop cleanup must clear its previous release error.
  Explicit release must preserve any separate unresolved deployment failure.
- Inspect/reconcile the exact retained allocation through public MCP. Confirm its
  release before removing runtime credentials or terminating compute. A changed
  account binding must be refused until the existing binding has been revoked,
  even after a container restart; neither private file may be overwritten.
- Exercise the crash window after durable remote release but before local journal
  cleanup. Remove the old provider binding and restart the service: the released
  identity must clean up without network/provider access. Unreleased, corrupt,
  nonprivate and reference-mismatched records must retain their holds. Keep this
  deterministic recovery test separate from genuine provider release evidence;
  never alter a live allocation journal to fabricate release.

### Existing New cloud dialog presentation

Retain the same title, repository, committed revision, profile and create/deploy
workflow. Check full-width fields, selected profile resources, visible Cancel/Create
actions, errors and scrolling at 4K and the native 800×600 minimum. Read the
actual window geometry: smaller requested widths may be clamped by the window
manager. Tab/Shift-Tab and
Space/Enter must stay in the modal; toolbar actions must not activate underneath.
Hold Escape through repeats and release: dismiss only the dialog, retaining cloud
fullscreen and terminal input isolation. Capture launch and resize plus a short
native recording. No issue picker or remote-device picker belongs in this flow.
Close the dialog, leave at least one ordinary frame, then reopen using its toolbar
button. Repeat Tab traversal and click the backdrop over Fit all: dismiss the
dialog without fitting the underlying canvas or activating other toolbar actions.
