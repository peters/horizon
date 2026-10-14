---
procedure: tailnet-marker4-rollout
feature: Published marker-4 CPU image and quick-start pin
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [provider API credential, SSH identity, optional dedicated test-tailnet auth key]
owner: peters
---

# Marker-4 CPU image rollout test procedure

## 1. Purpose

Qualify the immutable public CPU worker image and its exact quick-start report.
Keep the private-storage and unprivileged-agent checks strict. Record incompatible
provider storage as unsupported rather than changing permissions requirements.

## 2. Applicability

- Candidate: issue #1335 M0b, after tagged enrollment is merged.
- Image: the CPU base published by a successful main Worker images run.
- Provider smoke: an authorized CPU runtime that enforces private Unix modes.
- This procedure does not qualify a GPU profile, CUDA, hardware rendering,
  RunPod UDP/Mosh, node deletion or API credential management.

The epic deferred the RunPod UDP question to #1368. That does not replace the
[release flow](../../release-flow.md#update-the-quick-start-image) image and real
startup gates. The [quick-start procedure](cloud-quick-start.md) describes its
RunPod lane and explicitly excludes GPU. A compatible CPU-provider substitution
must be explicit, as in [PR #1560](https://github.com/peters/horizon/pull/1560).
Record which provider passed and which original tasks remain blocked or untested.
Do not present a Hetzner result as a passed RunPod procedure.

## 3. Preconditions

- Freeze the candidate and record its source, toolchain and executable hashes.
- Use the exact immutable digest and workflow report in `quick_start.rs`.
- Use a private evidence directory, empty Docker configuration and owned containers.
- CPU rental requires separate authorization, a cost cap and exact cleanup ledger.
- A selected-tailnet lane additionally requires a dedicated test tailnet, a tagged
  non-ephemeral key and `tag:horizon-worker` in `tagOwners`.

## 4. Tasks

### 4.1 I01 — Verify publication and image bytes

1. Record the successful main workflow, source SHA, helpers digest and CPU digest.

   Result: The image uses the exact published helpers and a main source revision
   containing tagged enrollment. A diagnostic or branch image does not qualify.

2. Pull the CPU image anonymously by immutable digest. Check its revision label.

   Result: The label matches the workflow source, and anonymous pulls work.

3. Run the image's own full built-in selection check with `--git-auth`, the current
   marker check and all-layer SSH-host-key check.

   Result: The report matches `CONTRACT` exactly. It contains only tailnet marker 4.
   No image layer contains an SSH host key. A static marker alone is insufficient.

4. Run the quick-start source and ignored image tests.

   ```sh
   cargo test -p horizon-core quick_start
   cargo test -p horizon-core quick_start -- --ignored --nocapture
   ```

   Result: The built-in profile remains CPU-only and image-only. Selected-tailnet
   qualification, stable names and all existing capabilities remain bound to the pin.

### 4.2 I02 — Verify strict isolation without real keys

1. Run a local image container with a compatible root-owned private workspace.

   Result: The isolated launcher runs as UID 10001 and cannot read root/private
   state. No auth key or actual tailnet is used.

2. Run a local image container whose mount root is writable by the agent.

   Result: Startup refuses before private state or enrollment is published.
   Do not relax root ownership, mode checks or sandbox protection to pass this step.

3. Perform the applicable image-source GitHub-chain lane C checks from the
   [release flow](../../release-flow.md#update-the-quick-start-image).

   Result: Actual service and isolation checks pass. Record valid reuse of unchanged
   helper scenarios with exact input comparison; mocks do not replace this gate.

### 4.3 P01 — Qualify actual CPU startup and cleanup

1. On the first reviewable candidate and final candidate, deploy the exact CPU pin
   through the public deployment path with a synthetic repository and **None**.

   Result: The worker reaches Ready without local Docker, image builds or registry
   credentials. Its services include GitHub; agents and private state remain isolated.

2. Exercise the affected reconnect/rebuild lifecycle and delete only ledger-owned
   resources. Verify actual provider absence.

   Result: The recorded image and workspace behavior match the candidate, and
   compute/storage/key cleanup is confirmed. Record limitations of a CLI-only run.

3. Record the selected provider and original quick-start tasks actually performed.

   Result: CPU results do not claim another provider, UI lane or GPU qualification.
   Prior evidence with a different pin does not qualify this candidate.

### 4.4 P02 — Record selected-tailnet evidence separately

1. If authorized, run R01 of the [tagged-enrollment procedure](tailnet-tagged-enrollment.md)
   on compatible CPU storage with the exact pin. Record its separate status.

   Result: Actual tags, key-free resume and preserved identity are observed. A
   keyless CPU startup or marker report alone does not prove real enrollment.

2. Remove only the node and resources of that run.

   Result: Cleanup preserves baseline nodes. Cloud deletion does not yet delete
   the tailnet node automatically.

### 4.5 N01 — Record incompatible storage

1. Examine the bounded, keyless storage diagnostic for the exact authorized runtime.

   Result: Record ownership, observed modes, successful descriptor/path operations,
   same-inode checks and cleanup. Do not include operational identifiers publicly.

2. For the tested RunPod persistent modes, record isolated startup as unsupported.

   Result: `0777` directories and `0666` files do not become private just because
   `chmod` returned success. The worker stays refused. Selecting **None**, ignoring
   metadata or relocating only a permission check is not a supported workaround.

## 5. Pass criteria

- The pin/report identify the actual main-published CPU image.
- Required source, image, isolation, GitHub-chain and compatible-provider gates pass
  on the applicable first/final inputs.
- Incompatible RunPod storage remains unsupported, with explicit negative evidence.
- P02's separate status is truthful; no marker or keyless lane substitutes for it.
- GPU qualification remains separate under [.horizon/README.md](../../../.horizon/README.md).
  CPU success cannot qualify the GPU profile, even if GPU rental stock was used for a diagnostic.
- Full local matrix, exact-head CI and independent review remain required.

## 6. Cleanup and evidence

Keep exact resource ledgers and private logs outside the repository. Remove only
owned containers, CPU resources and optional test nodes. Preserve uncertainty.
Record which checks were new, reused, blocked or untested. An interim report is
not final readiness, and a provider diagnostic image must never become the pin.
