# Tailnets MVP: local qualification, 3 October 2026

Auth keys and cloud panels only. OAuth, API administration and Remote Hosts are
follow-ups to [#1166](https://github.com/peters/horizon/issues/1166).

## Candidate

- Worktree: `issue-1166-cloud-tailnets`, based on `08415419f`.
- Frozen Linux UI SHA256: `14326c3ea03de1481eccc6cd2faacc8cbbb4ddf8b6b706849d82032e8ccce023`.
  Both actual application children matched this hash.
- Final minimal worker digest: `sha256:1900da40b7a35805e8bec78e63bac83fb70354cddfa88cb2a00eca2133680efc`.
  The resumed live worker's networking helper matched the final source.
- Ubuntu 24.04, approximately 538 MB image, no agents, browsers or desktop.
  Ubuntu's libc matches the worker helper requirements. The provider used a
  2-vCPU/4-GB server, its minimum 40-GB root disk, and a 10-GB persistent volume.

## Measured evidence

| Lane | Result |
| --- | --- |
| Workspace regression | 4,686 passed, 36 ignored, zero failed across 55 suites |
| Production Clippy | Library/binary/example checks with warnings, unwrap and expect denied passed |
| All-target Clippy | Warnings denied passed |
| Worker Python regression | 238 tests, 2 skipped, zero failures |
| Formatting and maintainability | Passed |
| Native UI | Dedicated Tailnets tab; named masked save/replace/remove flows, restart persistence and errors; dark/light themes and narrow/wide layout inspected |
| Provisioning UI | Multiple saved networks and None; cancelled creation starts nothing; allocated cloud shows a read-only provisioning choice |
| Native viewing | Connected native VNC panels, actual presentation and advancing frames on isolated desktops; 576-frame final-candidate video and 83-frame selection video fully decoded; representative feature frames inspected |
| Live MCP | Real agent panel; two saved entries returned as names/IDs only; selection schema present; raw keys and Stop selection refused without echo; no credential-reading tool |
| Selection fences | Core integration tests cover prepared selection, first-allocation confirmation, duplicate/lost-answer retry and refusal to change an allocated cloud |
| Real allocation | Minimal Linux cloud provisioned and this PC discovered in sanitized inventory |
| Cloud isolation | Agent UID 10001, no-new-privileges, zero capabilities; private node state, selection and daemon socket inaccessible; only public inventory fields visible |
| Persistence | Stop/resume retained the same Tailscale node without requiring the saved auth key |
| Daemon recovery | Killing the owned cloud daemon recovered in 8.9 seconds with the same identity; isolation and inventory rechecked |
| Worker compatibility | Final image imported a synthetic repository, prepared a shared checkout, committed and created a worktree as UID 10001 |
| Private TCP | Random nonce matched through cloud Tailscale Serve on permitted port 22, requested from this PC |

The real cloud saw itself and 42 ACL-visible peers, including this PC. This is
ACL-filtered discovery, not an administrative inventory of the entire tailnet.
HTTP proxy egress worked. Direct cloud-to-PC TCP and the alternative port 443
were blocked/timed out under existing policy. Configuring Serve on this PC was
refused by its local privilege policy. The successful private Serve test was in
the reverse direction, PC to cloud; it does **not** qualify arbitrary inbound
access to this PC. No ACL, firewall, operator setting or public Funnel was changed.
The cloud Serve configuration was restored to empty and its nonce backend removed.

The paid provider lane was Hetzner on Linux amd64. RunPod, other architectures,
macOS/Windows credential stores and graphics are not live-qualified by this run.
MCP's existing owner/checked-companion and first-allocation confirmation gates
remain; there is no general agent-controlled first-cloud creation tool. The
signed project-session runtime refuses selected tailnets until it has its own
qualified unprivileged launcher.

Long-running native capture also showed intermittent blank/glyph frames after
layout/theme interaction. The final feature was visible in the verified recording;
no terminal renderer changes or renderer qualification are claimed by this work.
Private screenshots, recordings, provider identifiers and network details are
excluded from this repository and the public issue.

## Cleanup

The owner approved deletion after the smoke. The cloud reached `Deleted`;
read-only provider queries confirmed the task's server, persistent volume and
labelled SSH key were absent. Its disposable network session had been logged out.
Task-owned canaries and the synthetic local worker container were stopped, and
private registry login material was removed. The shared PC's network policy was
unchanged. The auth-key-only scope and follow-ups are already in the issue;
publishing the later test results was blocked by an invalid CLI login and an
integration without issue-edit permission. The complete results are retained here.

## PR UI refresh

The final settings view removes the introductory credentials card and gives the
name/key fields consistent 12-pixel padding, 14-pixel text, eight-pixel corners
and larger spacing between labels, inputs and actions. Credential behavior is
unchanged. The refreshed frozen UI SHA256 is
`fb53b38b653652d8c2bcb10b3de9aef2f2558d3b15e46438254e6d2f78a1482c`;
the actual application child matched it.

A fresh isolated native VNC recording covers two synthetic saved networks, masked
save and replacement, an empty replacement-key field, selecting either network
or None in the real provisioning dialog, cancellation without allocation, and
removal. Full workspace and speech tests, formatting, maintainability and both
mandatory Clippy tiers passed again. Pedantic Clippy remains advisory and reports
existing excessive-bools warnings. No paid allocation was repeated for these
presentation-only changes.

## Review regressions

The final review-fix UI SHA256 is
`4c481af7b92f5e2322f9ac553d822c7ae48a4d1168d1b4d219530e1d81ee9231`;
the running child matched it. Native testing also verifies that pending changes
in another settings tab retain Save/Revert while Tailnets is open.

All 240 worker Python tests passed with the matching CLI, with no skips. An
offline container using the updated runtime prepared primary and sibling
checkouts through the actual unprivileged session launcher. The root-written
sibling manifest became readable by UID 10001. That UID could not read private
Tailscale state or the provider API key. Self-stop MCP registration survived the
privilege drop using only a nonsecret availability flag. Controller key-length
bounds now match worker validation; rejected retry selections leave the journal
unchanged; image validation qualifies the actual privilege-drop executable.

The complete required local matrix passed again. These regressions were tested
locally; the earlier paid discovery and connectivity evidence remains the real
provider qualification described above.


## Credential boundary regression

The public credential-reading callback was removed. Shared cloud storage exposes
metadata and credential writes/deletion only; the private host deployment adapter
alone reads a saved key and sends it through pinned, silent transport. A
compile-fail regression prevents reintroducing the old capturing callback.
A real Secret Service round trip on a disposable private bus verified saving,
private deployment reads, replacement and deletion with synthetic keys only.
The native feature recording also verified two named bindings, masked replacement,
provisioning choices for both bindings and None, cancellation and removal.
The recorded application child SHA256 is
`2aeaecb33080f9ad81d98bbac53c4c7e6d5bfccd56805a040e63a0d8bd25251e`.
The earlier real-cloud enrollment path is unchanged apart from moving key retrieval
behind this private boundary; no new paid allocation was made in this regression.

The final integrated branch passed formatting, maintainability, 4,703 workspace
tests and the speech tier, plus both mandatory Clippy tiers. The 38 workspace
ignores include external integration fixtures; the new private-keyring fixture
was run explicitly and passed. All 240 worker tests passed without skips.
Pedantic Clippy retains the existing advisory excessive-bools finding.


## Interrupted persistence regression

Credential updates now use fresh OS-store slots and a durable nonsecret journal.
Six focused regressions cover journal-only and post-key-write interruptions,
before/after catalog publication, idempotent cleanup, failed key writes, failed
catalog rename, cleanup failure/retry, deletion and legacy binding migration.
The real isolated Secret Service test verifies that replacement retires the old
credential while retaining the binding ID. Native metadata observations verify
that only the replaced network advances its credential generation, with no
pending journal after success. Public catalog serialization excludes slot data.
The refreshed native application child SHA256 is
`76929fc60134caa0315fdbae914c8fa71cf4dda2b90dd0c9d7678a20cc5e618a`.

The required final matrix passed again: 4,709 workspace tests, 1,579 speech-tier
tests, formatting, maintainability and both mandatory Clippy tiers. All 240
matching-helper worker Python tests passed without skips.


## Slow resume and active-edit deletion

Transient daemon states now fail retryably instead of requesting an auth key.
Only explicit `NeedsLogin` permits enrollment; a slow `Starting` reconnect,
unknown state or pending machine approval cannot reuse a consumed one-time key.
Regressions cover all non-login states, with and without a supplied key, followed
by a successful same-identity retry. Removing an edited binding clears its form
only after successful deletion; a failed deletion retains the edit.

The required matrix passed: 4,710 workspace tests, 1,580 speech-tier tests,
formatting, maintainability and both mandatory Clippy tiers. All 242 matching-helper
worker tests passed without skips. The isolated Secret Service test and actual
offline container isolation/sibling/self-stop smoke passed again.

The refreshed live native VNC flow verified two saved networks, empty/masked
replacement, stable binding IDs with one new credential generation, provisioning
choices for both networks and None, cancellation, deletion during active editing
and an empty final catalog with no pending journal. The running child SHA256 was
`48c7f8ed7933dc65052431b36b05cfd3a7a80b863b9dd2f7777e637264fcfe8e`.
A new continuous native recording covers this final candidate. No new paid cloud
allocation was made; the earlier discovery/connectivity proof retains its stated
policy and platform limits.

The final worker-only review refresh corrects the public status command so only
`Running` reports joined, and derives storage-probe failure diagnostics from the
actual privileged/unprivileged probe directory. All 244 worker tests passed with
no skips. A disposable offline container verified the actual status subprocess for
Running, NeedsLogin and Starting, plus agent isolation, sibling preparation and
self-stop availability. These changes do not alter the UI or recorded flow.


## Pending choice and concurrent inventory regression

Ensure Ready captures both explicit and default selections in a durable nonsecret
request. Pending requests reject UI/CLI edits and selection drift before companion
execution or deployment initialization. Regressions cover a saved network, None,
omitted selection, cancellation and failure before any backend execution. Existing
companion lifecycle and recovery tests all passed.

Inventory publishers use unique atomic temporary files. Concurrent writer and
failed-write regressions preserve a complete final snapshot and clean temporary
files. The real isolated prelogin daemon passed 20 concurrent publications without
an auth key; its prelogin self placeholder has no addresses or online status.
The actual offline agent isolation, sibling preparation and self-stop smoke passed.
The full matrix passed 4,711 workspace tests and 1,580 speech-tier tests, formatting,
maintainability and both mandatory Clippy tiers; all 246 worker tests passed without
skips. The isolated real Secret Service regression passed again.

The refreshed native candidate child SHA256 is
`4e8ddb8355c4ba3108d6d55029ed64000387933d2fb8f234ecea3151db13e920`.
Live native VNC and continuous recording verify the final settings fields, two
saved bindings, masked replacement with stable IDs, both provisioning choices and
None, cancellation and removal while editing. No new paid allocation was made.


## Probe environment and ownership follow-up

Isolated idle-watcher tmux clients clear the environment while still root, before
using `setpriv` with UID 10001, no new privileges and no capabilities. A real
network-disabled container verified that synthetic provider and tailnet secrets
are absent from both the child environment and `/proc/self/environ`; self-stop
availability and private-state isolation still work.

Workspace ownership migration is serialized and recorded in versioned private persistent
state only after success. Repeated launches hand off new primary and sibling
uploads and configuration files without scanning existing checkout trees. Actual
repeated handoff performed zero recursive ownership commands while transferring
both upload types. Failure/retry regressions keep incomplete migration retryable.
The real prelogin Tailscale daemon again published 20 concurrent complete sanitized
inventories without an auth key. The UI executable and verified native recording
are unchanged by these worker-only fixes.

All 248 matching-helper worker tests passed without skips. The full required
matrix passed again: 4,711 workspace and 1,580 speech-tier tests, formatting,
maintainability and both mandatory Clippy tiers. The existing advisory provider
boolean warning remains unchanged. The final speech-enabled application SHA256
still matches the native fixture and showcase above.


## Interrupted submission and container-resume follow-up

Tailnet submission stages only nonsecret request metadata before source persistence.
The target selection changes after the source intent and target claim are both
durable. A regression simulates exit after staging and an actual source-journal
size-limit refusal; neither changes the target selection, and a later omitted
choice remains None. Incomplete claimed submissions stay fenced and can be
cancelled through the existing unstarted-operation recovery path.

The versioned ownership completion marker is root-only on the persistent workspace
volume. Runtime recreation preserved it and caused zero recursive ownership scans
while handing off new primary and sibling uploads. The real offline probe again
verified UID 10001 credential isolation, and the real prelogin daemon passed 20
concurrent sanitized inventory publications without enrollment.


The final exact-checkout matrix passed 4,712 workspace tests, 1,580 speech-tier
tests, all 248 matching-helper worker tests, formatting, maintainability and both
mandatory Clippy tiers. The existing provider boolean warning remains advisory.
The frozen final native application child SHA256 is
`4750c412b2d07572d5440585b159ee3f7afe7097de4ea7eba823c108d0190b90`.
Three timestamped public native-viewer inspections confirmed displayed advancing
frames. The final 657-frame continuous recording verifies two saved networks,
empty/masked replacement, stable binding IDs with one new credential generation,
Office/Lab/None provisioning choices, cancellation and clearing an edited removed
binding. Both test bindings were deleted with no pending credential journal; all
owned desktop children exited and target files expired. The showcase retains
original native frames around 30 verified feature actions and omits idle waits
and unrelated tab navigation. No new paid cloud was allocated.

## Privileged-write regression

The controller writes agent credentials after dropping to UID 10001, using
unique mode-0600 temporary files and atomic replacement. Root SSH/SCP uploads
siblings into root-only staging; only the completed upload's consumer passes
read-only descriptors into an environment-cleared UID-10001 process. Ordinary
panel launches leave unfinished transfers untouched. Sibling manifests live in
the agent-owned `.horizon` directory on isolated workers.

A network-disabled disposable worker tested actual compiled credential commands
with synthetic secrets, leaf links to root-only files, an agent-owned sibling
parent pointing at a root-only directory, and primary/sibling pack/archive
handoff. Protected files stayed unchanged; valid destination files belonged to
UID 10001, failed handoffs retained their input, and resumed ownership migration
performed no recursive scans. A real loopback OpenSSH/SCP transfer followed by
Git import verified the private stage path and unprivileged import with a
malicious destination link. No host credentials or external network were used.

The final required matrix passed 4,712 workspace tests, 1,580 speech-tier tests
and all 250 worker tests with no skips. Formatting, maintainability and both
mandatory Clippy tiers passed; the existing provider boolean warning remains
advisory. The actual native application child matched the frozen speech-enabled
SHA256 `1ffbdb3ef0cdffc229b25c5e4fd8b811df42ba83412d929249084fcaf0fa027a`.
Public native-viewer observations established displayed advancing frames. The
334-frame recording covers two bindings, masked replacement with stable IDs,
Office/Lab/None provisioning selection, cancellation, removal while editing and
Fit. Both synthetic bindings were removed without a pending journal; all owned
processes exited and the device target expired. The showcase retains original
frames around 32 feature actions. No new paid allocation was made.
