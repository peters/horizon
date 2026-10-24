---
procedure: portable-image-publishing
feature: Portable worker image publishing defaults and setup
platforms: [linux, macos]
cost: none
destructive: no
secrets: synthetic-only
owner: peters
---

# Portable image publishing

## 1. Purpose

Check the ordinary image-omitting build flow, old staging settings and portable
setup guidance. Keep metadata, actual publication, native presentation and paid
worker allocation as separate evidence.

## 2. Applicability

Use a supported Unix host and an owned Docker engine. The native fixture runs on
Linux X11. Hosted CI checks cross-platform parsing and compilation. GPU allocation
is not qualified by this procedure.

Local registry proof uses synthetic accounts. It does not establish live GitHub
personal or organization permission, or GitLab account connection. Git hosting
and image hosting are independent; no GitLab connector is added.

## 3. Safety

Use only task-owned source clones, private settings, local registry accounts,
containers and volumes. Do not change installed account bindings, use a real
device sign-in, allocate compute or trigger a release. Never prune shared Docker
resources. Keep credentials outside source, images, build contexts and reports.

## 4. Equipment and preconditions

- A frozen candidate, source inventory, exact tree and executable hashes.
- An isolated native desktop with a public Horizon Device viewer.
- A reviewed public source clone with a committed build profile that omits image.
- Private registry writers and separate repository-scoped read-only readers for
  two synthetic accounts, with an exact owned-resource ledger.
- The resource capacity required by the setup gate: at least 100 GB free at its
  output and Docker roots, and a load average below 30.

## 5. Setup

1. Inspect the source repository's CI, Dockerfile and pins before execution.

   Result: The public input is reviewed. Removing only its publication image
   leaves its existing toolchain, proof jobs, runtime and build needs intact.

2. Use ordinary private cloud settings without the retired staging key.

   Result: Publishing metadata and controls are available. Repeat with an old
   false value. Reading does not rewrite it or touch credential material.

3. Follow the native and registry procedures linked below.

   Result: Owned targets, process identities, source inputs and cleanup ledgers
   are recorded before the interaction or publication starts.

## 6. Tasks

### 6.1 PP-01 — Defaults and preserved settings

1. Run the focused settings and management tests.

   ```sh
   cargo test -p horizon-core --lib legacy_false_does_not_disable_completed_publishing
   cargo test -p horizon-core --lib legacy_staging_false_does_not_block_a_confirmed_personal_choice
   cargo test -p horizon-ui --bin horizon legacy_staging_false_still_requires_current_publishing_confirmation
   ```

   Result: Fresh settings and setup drafts enable the flow. Missing, false and
   true legacy values all load enabled without rewriting bytes. Invalid legacy
   types refuse parsing. A later save omits the obsolete key and preserves
   unrelated credentials. A stale publisher still refuses before a first record;
   retiring a staging value grants no account or destination authority.

2. Run the applicable full local matrix and the skills coverage check.

   Result: Blocking and strict tiers pass. Hosted checks and independent review
   cover the exact PR head. Record new, reused and non-applicable results separately.

### 6.2 PP-02 — Ordinary UI, CLI and MCP

1. Run [the selector procedure](image-destination-selector.md) without a staging
   switch and repeat with the old false value.

   Result: The destination and publishing account appear before Start. A saved
   registry retains its exact grant; reset restores profile intent. Save and
   return and Cancel return from configured registry settings without starting.
   Storage changes preserve owned sign-in, while queued Start still pins its
   complete profile. Dark, light and minimum-width layouts remain usable.

2. Run [the public operation procedure](image-destination-public.md) through
   actual local agent panels, normal CLI plans and stdio MCP.

   Result: List/select/reset, stale snapshots, cross-actor receipts and
   cancellation behave alike. Old false settings do not hide the tool. The next
   explicit save drops the obsolete key and keeps credential references. YAML is
   unchanged. Existing clouds report their retained metadata through cloud_list.

3. Finalize the native video and inspect decoded frames before making the PR GIF.

   Result: Live presentation and motion have separate public inspections. The GIF
   shows generic fixtures only and identifies the final candidate binary hash.

### 6.3 PP-03 — Real source and setup proof

1. Inspect the setup prompt and its declared profile.

   Result: New build profiles omit image. Existing explicit destinations or
   omissions are preserved unless requested otherwise. Prebuilt profiles require
   image. Git origin and source authors never choose publishing credentials.

2. On the first and final reviewable heads, run the
   [real setup gate](cloud-setup-gate.md) with the committed public input and the
   candidate's qualified test binary. Use a new output directory each time.

   ```sh
   python3 scripts/setup-gate.py LIST.json --lanes 1 --binary TEST_BINARY --out EVIDENCE/setup-first
   ```

   Result: Build image, CI in image and the warm rerun pass. Source YAML omits the
   publication target. The selected CI jobs run with event push. A CLI-only source
   has no desktop preview; record that as non-applicable. This proves the declared
   setup, not a fresh run of the setup agent or a rented worker.

3. Repeat the read-only real-checkout gates in
   [destination management](image-destination-management.md) and
   [public operations](image-destination-public.md).

   Result: The actual source resolves correctly. YAML and private fixture settings
   remain unchanged and no credential or provider operation starts.

### 6.4 PP-04 — First and final local publication

1. On both reviewable heads, run the fresh-state two-publisher checks in
   [the build engine procedure](portable-build-engine.md).

   Result: The same committed image-omitting source builds and pushes through two
   separate authorized local destinations. Each starts with a new state directory;
   a cached built record cannot substitute for initial publication.

2. Verify immutable reads with the separate scoped readers, then change current
   preferences and exercise retained retry and rebuild checks.

   Result: Destination, publisher and grant remain the original recorded values.
   Wrong grants refuse before expensive work. Read-only credentials remain
   separate from publishing credentials. No provider allocation occurs.

## 7. Pass criteria

All applicable tasks pass on the final executable inputs. Development profiles
omit only the personal publication property; public base pins, CPU/GPU needs and
agent protections remain intact. Do not count local registry proof as live issuer,
GPU or paid-allocation evidence.

## 8. Cleanup

1. Close the owned native app normally and release its viewer.

   Result: Its process tree exits, private home disappears and target expires.

2. Verify the setup gate cleanup, then remove exact local registry resources from
   their owned ledger and delete private fixture credentials.

   Result: No task container, volume, listener or credential remains. Unrelated
   Docker resources, installed settings and running applications are unchanged.

## 9. Record of results

Record the guarded head, tree, input commit, commands, logs, binary hashes,
first/final setup and publication evidence, native presentation and cleanup.
State unsupported or non-applicable lanes and which real services were not used.
