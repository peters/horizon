---
procedure: image-destination-selector
feature: review image publishing before creation
platforms: [linux]
cost: none
destructive: no
secrets: synthetic-only
owner: peters
---

# Image destination selector

## 1. Purpose

Verify the gated New cloud destination review and its retained publisher choice.

## 2. Applicability

The selector is staged behind machine-local `portable_image_publishing`, false by
default. This native fixture runs on Linux with X11. Cross-platform Rust checks
are separate evidence; native macOS qualification remains deferred under the
local device-smoke workflow. Windows cloud operations are unsupported. This
procedure qualifies no registry push or worker allocation.

## 3. Safety

Use only the owned isolated desktop and synthetic account metadata. Do not sign in
to a real account, change installed credentials, start a worker or allocate compute.

## 4. Equipment and preconditions

- A frozen candidate binary, its SHA-256 and source input inventory.
- A task-owned native desktop and public Horizon Device panel in the current workspace.
- Private application state and a real synthetic Git checkout with a committed build
  profile that omits `image`.
- Synthetic account metadata and registry bindings. No provider keys, real registry
  secrets or usable SSH identity.

## 5. Setup

1. Follow `scripts/device-smoke/README.md` and create the owned native viewer.

   Result: Three public inspections two seconds apart show displayed frames and
   advancing sequences while the fixture heartbeat changes. Record the child
   executable path and hash.

2. Enable the staging gate only in the fixture's private settings.

   Result: The user's settings, processes and workers are unchanged.

## 6. Tasks

### 6.1 IS-01 — Review and change the destination

1. Start viewer video and open New cloud for the synthetic checkout.

   Result: Publish images shows the image URI and publishing account. Start remains
   blocked by missing compute prerequisites.

2. Use Change and choose the configured synthetic registry.

   Result: The URI and publisher change. Only the private machine preference changes;
   committed YAML remains unchanged.

3. Cancel creation and reopen the same checkout and profile.

   Result: The saved registry choice returns. Use profile default removes that local
   preference. Automatic requests sign-in when no account is cached.

### 6.2 IS-02 — Separate pull access and account preparation

1. Load a synthetic personal choice without a pull binding.

   Result: The account and target remain visible, Set up worker pull access is offered,
   and Start is blocked. The settings action targets that exact repository. Its
   description does not claim the destination came from YAML. A cached sign-in is
   identified as metadata, not proof of publishing access. The hint does not
   promise account selection at the first push. On a machine with valid compute
   and SSH settings, the dialog remains open. **Save and return** and Cancel
   reopen New Cloud for a fresh destination review; neither starts a worker.
   Run `cargo test -p horizon-ui registry_edit_stays_open --bin horizon` to
   exercise configured settings, save completion and cancellation.
   Run `cargo test -p horizon-ui registry_edit_preserves_agent_bindings --bin horizon`
   with missing credentials for an agent required only by the profile. Registry
   edits save with the machine's agent defaults and retain the existing credential
   references. The next Start still requires the profile's missing credential.
   This opens the full Cloud settings form: its usual validation still requires
   credentials for selected machine-default agents. Fix a missing default-agent
   credential there before saving; an agent required only by the profile does not
   block the registry edit.
   Run `cargo test -p horizon-ui registry_edit_invalidates_readiness --bin horizon`.
   A cached ready profile, including a read finishing after the settings edit,
   cannot skip the next Start's current account validation.

2. Run `cargo test -p horizon-ui creation::publishing --bin horizon`.

   Result: Scope changes discard pending answers; disconnected jobs clear their
   codes; coalesced prompt completion settles; refresh preserves visible metadata
   while blocking dispatch. During a bounded background reload, Start can queue
   an already shown valid choice. First reads cannot queue unknown choices, and
   dispatch waits for shared policy to return the same choice. Cancelling a sign-in ends only its
   owned attempt and permits a fresh retry. Start cannot queue during a destination save or sign-in,
   before or after its prompt arrives; its code, link and Cancel stay usable.
   Opening the verification page uses a worker thread and reports failure
   separately from metadata. Repeated clicks cannot start concurrent openers.
   A disabled gate preserves explicit
   recipe behavior.
   Storage-only edits keep an owned metadata read or sign-in alive and retain
   its code and cancellation handle. They do not reuse a queued Start's older
   full-profile confirmation. All other scope changes still discard old answers.

3. Run `cargo test -p horizon-ui a_queued_build_waits_for_destination_refresh_without_losing_start --bin horizon`
   and `cargo test -p horizon-ui creation::watch --bin horizon`.

   Result: The actual queued dispatcher and stock watch retain their requests
   during refresh. Start and the stock checkbox remain available for an already
   shown valid choice, and Enter can queue that choice during refresh. A changed
   publisher or profile requires a new review. A
   build-to-prebuilt change never forwards the previous publisher. Start and
   Enter share the same reviewed confirmation path. A stopped watch keeps its
   reason after refresh; the original metadata error remains visible separately. A
   watch waits while a reload is in progress. If that reload fails, the watch
   stops and requires a new review; it does not silently rearm after recovery.

4. Do not perform real device authorization in this synthetic smoke.

   Result: No real account or credential is changed. Authentication itself is covered
   by the separate publication credential procedure.

### 6.3 IS-03 — Persist reviewed choices safely

1. Run the historical launch-context regressions.

   ```sh
   cargo test -p horizon-ui --bin horizon historical_build_choices_can_change
   cargo test -p horizon-core --lib prepared_historical_profile_keeps
   ```

   Result: A real two-commit checkout selects an older image-omitting build after
   the current profile is removed or made prebuilt. Actual selector selection and
   reset settle against that historical profile. Shared mocked account sign-in
   uses the same prepared profile and retains all grant and stale-choice checks.
   YAML is unchanged; no credential or real authentication is used.

2. Run `cargo test -p horizon-core cloud_panel::launch_serde --lib`.

   Result: A reviewed actor round-trips in a version 2 envelope. The old flat reader
   refuses it; missing choices, unsupported versions and unversioned choices fail.
   Legacy launches retain their previous flat encoding. Recipe-only confirmations remain flat and compatible with the older reader. Both JSON and YAML round trips pass.

3. Run `cargo test -p horizon-core cloud_runtime::setup::tests::metadata_loading --lib`.

   Result: First-use metadata reads create no settings or credentials. Malformed
   existing settings are refused without replacement.

### 6.4 IS-04 — Layout and cleanup

1. Repeat the destination flow in light and dark themes and at minimum window width.

   Result: Targets wrap, controls stay reachable, and changing a choice preserves
   its visible location while refreshed metadata loads. The loading indicator
   appears on the first read and keeps the same space after loading.

2. Stop and finalize video; inspect representative decoded frames.

   Result: The recording has advancing frames and no encoder failure. Retain the
   candidate input inventory and motion evidence together.

3. Close the owned test application and viewer, then stop its harness.

   Result: Its children exit and target expires. The user's Horizon stays running.

## 7. Pass criteria

All applicable tasks pass on the candidate's executable inputs. Local metadata and
synthetic state-machine checks do not prove live issuer permission or publication.

## 8. Cleanup

1. Close the owned application normally, close its public viewer and stop its harness.

   Result: Task-owned children exit and the private home is removed. User sessions
   and accounts remain unchanged. Retain finalized video and logs outside that home.

## 9. Record of results

Record the exact candidate, binary hash, first/final real source checks, native
session and normal-close evidence, video/GIF, local check logs and cross-platform
CI. Keep unexecuted platforms and real authentication separate from synthetic proof.
