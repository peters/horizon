---
procedure: tailscale-unix-proxy
feature: Pinned Tailscale Serve Unix PROXY v1 runtime
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Tailscale Unix PROXY test procedure

## 1. Purpose

Test the pinned source patch and both native runtime images.
Check header ordering and forwarding through the actual Serve handler.

## 2. Applicability

- Use the candidate recipes on native Linux amd64 and arm64.
- This procedure grants no exposure or enrollment authority.
- Consumer identity, Native controls and M2 require separate real tests.

## 3. Safety

Use task-owned image tags, directories and test sockets.
Do not mount credentials, existing daemon sockets or production tailnets.
Do not publish an image during a local check.

## 4. Equipment and preconditions

- The candidate checkout, Docker and the build prerequisites.
- The exact upstream archive and Go image pins in the runtime README.
- Separate native architecture runners and sufficient build storage.

## 5. Setup

1. Record the source patch, archive hash, compiler index and architecture.

   Result: The inputs match the candidate recipes.

2. Reserve the build lane and storage before the image build.

   Result: The run cannot remove another task's cache or exceed its assigned limit.

## 6. Tasks

### 6.1 U01 — Upstream behavior

1. Build the `tailscale-runtime` target in the standalone Dockerfile.

   Result: Both affected upstream Go test packages pass before binaries are built.

2. Read `TestServeUnixProxyProtocol` and its raw result.

   Result: Unix v1 headers use the actual source and virtual port for IPv4,
   IPv6 and mapped IPv4. PING and PONG cross the same stream after the header.
   Unix without PROXY remains valid. Unix v2, zero port and missing node address
   refuse before payload. TCP v1 and v2 keep their backend port and header bytes.

3. Read the CLI mutation tests and `TestServeUnixProxyV2PreservesConfig` result.

   Result: Unix v1 saves the intended route. Unix v2 refuses and preserves it.

### 6.2 U02 — Images and notices

1. Run `examples/cloud-worker/build-helpers-image.sh <full-candidate-commit> <task-tag>`
   on each native architecture.

   Result: The helpers probe passes with the patched binaries from that same build.

2. Run `examples/cloud-worker/build-base-image.sh <task-tag> <task-base-tag>`.

   Result: The current worker markers pass on each architecture.

3. Export the two Tailscale binaries and `/usr/local/share/licenses/tailscale`.

   Result: The version identifies the patch hash. The source hash, exact patch,
   upstream license, Go notices and consumed module notices are present.
   Go, module caches and source are absent from the final image.

4. Compare two independent builds with identical inputs.

   Result: Record the two binary hashes. Report reproducibility only if they match.

### 6.3 U03 — Repository integration

1. Run the affected worker Python suite, skills coverage, maintainability,
   documentation and PR readiness checks.

   Result: Generated CPU/GPU contexts contain exactly the declared runtime inputs
   and exclude undeclared files. Both bundled skills describe the boundary.

## 7. Pass criteria

U01, U02 and U03 pass on both native architectures where applicable.
No result grants runtime identity or completes the subsequent Unix consumer.
No new image pin is claimed before an actual publication.

## 8. Cleanup

1. Remove only the run's exact image tags, test sockets and temporary directories.

   Result: Original workers, credentials, daemon sockets and shared caches remain.

## 9. Record of results

Record exact commits, uncommitted inputs, commands, architecture, raw results,
source and binary hashes, image digests and cleanup in the PR or a retained report.
Separate new, reused, pending and non-applicable checks.
