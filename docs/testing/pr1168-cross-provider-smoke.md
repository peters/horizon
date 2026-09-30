# PR #1168 shared cloud picker smoke plan

Execute against the final candidate checkout and frozen debug executable. Record
commit, executable SHA-256, exact application PID, platform and runtime assumptions.
Use generic synthetic repository content only. Run all interactive scenarios on a
task-owned isolated desktop through a live native VNC Device panel in the calling
workspace. Record timestamped connection/received/displayed/advancing-frame evidence
before input. Record a scoped video before the flows and inspect decoded frames;
create the PR GIF with a two-pass palette under 10 MB. Never allocate a paid worker.

## Setup and baseline

1. Run the entire repository pre-push matrix in this checkout, including speech,
   blocking/strict Clippy, maintainability and advisory pedantic. Run cloud_deploy
   example tests and build the device CLI and debug Horizon binary.
2. Copy the binaries to a new private evidence directory and hash them. Start the
   documented device-smoke fixture with --native-view and private home/state.
   Seed a synthetic repository with CPU minimums 8 vCPU/32 GB and GPU minimum
   24 GB VRAM, 30/50 GB container disk and 80/100 GB workspace storage.
3. Copy only required read-only provider credential bindings to private 0600 files.
   Keep credentials and real operational identifiers out of every public artifact.
   Verify actual child executable hash. Create the task-owned native Device panel;
   inspect at least three times, two seconds apart while the fixture heartbeat changes.
4. Observe launch layout, open New cloud and load the repository. Confirm filters
   visible on the first frame, In stock only checked, below requirements unchecked.

## Primary flows

1. CPU: both supported configured providers appear together. Cards and rows include
   provider identity; Hetzner includes exact type/location and EUR billing price.
   Estimated USD totals and exchange date are visible. Cheapest satisfies the profile.
2. Narrow All providers to RunPod and Hetzner in turn; cards and list agree. Restore
   All providers. Search by provider/type/resources and clear it. No result gives
   useful guidance. Sold-out workers appear when stock filter is unchecked.
3. Reveal below-minimum workers: explain rejection and disable selection. Matching
   choices remain selectable. Profile changes restore filter defaults.
4. Choose a Hetzner row/card; summary has exact type/location, native EUR running
   estimate, workspace and IPv4 costs. Change run length and workspace size; totals
   update without losing the explicit choice. Switch back to RunPod: previous Hetzner
   exact type/location constraints must be cleared. Cancel must not allocate.
5. GPU: only GPU-capable providers participate. Minimum VRAM holds, selected exact GPU
   type remains displayed, unavailable workers can be inspected, waiting requires
   the explicit availability option. Hetzner errors cannot block GPU picks.
6. Resize the isolated window smaller and larger and exercise scroll/fit. Capture
   screenshots after launch, normal selection and resize; action bar and filters stay
   reachable with no clipped critical controls. Observe motion in the scoped recording.

## Edge cases and parity

1. Unit/integration tests: catalog timeout/failure and stale FX prevent a global
   cheapest claim but preserve native offers and explicit selection. Refresh invalidates
   freshness. Single supported/provider-filtered lane may still rank its available offers.
2. Provider policy filters allowed types/locations consistently in UI, CLI and MCP.
   Container disk minimums apply to both providers. Unknown rates stay uncomparable.
3. Exact Hetzner choice survives serialization, plain-settings resume/reconnect/rebuild
   and image refresh; removed type/location is rejected without another allocation.
   Legacy saved workers retain their documented fallback policy.
4. CLI offers is read-only, retains native sections and adds dated USD comparison.
   --worker-choice validates one bounded file and resolves identity against current
   catalog/policy; forged resource metadata must not bypass profile minimums.
5. MCP cloud_offers uses the same ranking and policy without requiring settings UI
   open. Worker snapshots with an absent provider catalog remain incomplete until
   explicit unconfigured evidence arrives. Short deadlines do not trigger FX HTTP.
6. Relaunch the isolated candidate and confirm legacy settings/profiles load. Confirm
   cancellation leaves no workers or operational state changes. Close only the owned
   window/viewer and fixture, verify endpoint expires and children exit.

## Report

Record each lane pass/fail/blocked with exact head/hash and evidence paths. Separate
native UI coverage from unit lifecycle coverage and read-only network coverage. Do
not claim a provider's live lane without its binding. Complete all applicable lanes
before reporting ready; remove this temporary plan after successful UI validation.
