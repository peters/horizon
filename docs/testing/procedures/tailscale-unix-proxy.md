---
procedure: tailscale-unix-proxy
feature: Pinned Tailscale Serve Unix PROXY v1 and explicit flow cancellation
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Tailscale Unix PROXY test procedure

## 1. Purpose

Qualify the pinned runtime's explicit descriptor, half-close and drain-ACK
contract through actual Serve handlers and both native runtime images.
Record later consumer qualification separately.

## 2. Applicability

- Native Linux amd64 and arm64 candidate builds; non-Linux opt-in must refuse.
- `--horizon-flow-cancel` and JSON `HorizonFlowCancel` default false.
- No enrollment, policy, publication, Native or exposure authority is granted.

## 3. Safety

Use task-owned image tags, directories and test sockets. Do not mount credentials,
existing daemon sockets or production tailnets. Do not publish during a local
check. Real Kernel/Worker consumer runs require their own original-custody plan;
this procedure does not authorize resources or enrollments.

## 4. Equipment and preconditions

- Candidate source, exact upstream archive/Go pins and available build storage.
- Separate native architecture runners and Docker build prerequisites.
- Raw results for current candidate inputs; older non-descriptor passes are
  historical evidence, not the new capability's qualification.

## 5. Setup

1. Record candidate/source-patch hashes, archive/compiler pins and architecture.

   Result: Both image recipes use the same checked patch and module checksums.

2. Reserve the build lane and storage before running the declared image recipes.

   Result: No foreign cache is removed or resource limit exceeded.

## 6. Tasks

### 6.1 U01 — Opt-in and config preservation

1. Build the `tailscale-runtime` target and retain complete raw results for
   `go test -p 6 -timeout 10m ./ipn ./ipn/ipnlocal ./cmd/tailscale/cli`.

   Result: All three complete upstream packages pass before binaries are built.
   Generated clone/view consistency and JSON/status round trips retain the field;
   false/absent omits it and retains ordinary handler behavior.

2. Exercise CLI and direct LocalAPI proposed updates at top-level TCP, nested
   Foreground and Services TCP entries, plus restored handler validation.

   Result: Only Linux raw TCP, absolute filesystem Unix, PROXY1 and nonzero
   ingress accept opt-in. HTTP/HTTPS, TLS termination, Funnel, relative targets,
   invalid port/protocol and unsupported platform refuse without replacing the
   last valid config. Test matching-port Funnel across config scopes, direct API
   updates and actual ingress; a raw opted-in transport must not accept it.
   Ordinary TCP v1/v2, Unix without PROXY and Unix PROXY1
   remain unchanged. Configuration responses and normal `serve status` node
   and service trees show `Horizon flow cancellation` only for enabled handlers.
   Disabled text remains identical, including ordinary TCP/TLS. Enabled Unix
   targets have no TCP prefix.

3. On a tagged fixture with no service configuration or advertisement, reject
   `serve --service=svc:new --tun --horizon-flow-cancel` and the other invalid
   opt-in combinations. Also reject conflicts in the shared node/Foreground
   port space before any service preference change.

   Result: No panic, preference edit, persisted config change or HTTPS enablement
   occurs. Status generation runs only after a successful config mutation.
   Service VIP ports retain their distinct routing namespace.

4. Attempt raw opt-in on an existing TUN service and over same-port TLS, HTTP or
   HTTPS handlers, with the original service both drained and advertised. Also
   replace a raw handler and add raw forwarding beside a different-port handler.

   Result: Each incompatible transition refuses before preference/config writes,
   leaves advertisements and original configuration unchanged, and prints no
   success. Valid raw replacement and different-port coexistence retain normal
   advertisement behavior. Recursive opted-in TUN handlers refuse; ordinary
   false/absent handlers and the original preference-error behavior are unchanged.

### 6.2 U02 — Actual handler and original-flow retirement

1. Run real TCP-to-Unix and userspace gonet handler fixtures with ancillary receive.

   Result: Exactly the first `P` carries one cancellation FD; the remainder is
   the correct source-family/virtual-port v1 header before untouched payload.
   The sender's local duplicate closes after transfer; a failed/ambiguous send
   never retries. Preserve the earlier first-copy regression's expected failure.

2. Exercise request write EOF followed by a delayed response, reverse EOF while
   request input stays writable, simultaneous EOF, and a response larger than
   frontend receive capacity. Assert the real frontend CloseWrite executes.

   Result: Both payload directions drain without truncation. Control remains
   live during payload half-close. Exactly `0x01` then EOF occurs on control only
   after both copies and CloseWrite calls succeed; external response bytes and
   EOF are observed before declaring fixture success. ACK is not app delivery.

3. Cancel by control close/death and unexpected consumer control data while
   idle, header-blocked, backend-write-blocked and response-write-blocked;
   exercise daemon shutdown, header short/error writes, copy/read errors,
   unsupported CloseWrite and failed/short ACK.

   Result: Original payload directions close promptly, failure emits no success
   ACK, no ambiguous transfer/ACK is retried, and all started sender/copy/control
   reader goroutines join. Capture FD ownership/absence, including temporary
   conversions and sent-end duplicates. No result sender remains blocked.
   Cancellation before terminal success wins; induced close errors do not erase
   the first meaningful failure. No production timeout is enlarged.

4. Pause completion before and after its synchronized success transition, and
   record consumer cancellation while paused. Observe the actual control socket.

   Result: Cancellation recorded before commit produces an error and EOF without
   an ACK. After commit, `0x01` plus EOF and the daemon result agree on success;
   cleanup reader errors cannot change that result. The consumer independently
   applies its original cancellation/deadline checks before accepting success.

### 6.3 U03 — Native images, notices and reproducibility

1. Run `examples/cloud-worker/build-helpers-image.sh <full-candidate-commit> <task-tag>`
   on each native architecture, then
   `examples/cloud-worker/build-base-image.sh <task-tag> <task-base-tag>`.

   Result: Current complete three-package tests, helpers probe and worker markers
   pass with the binaries from that same build on amd64 and arm64.

2. Export both Tailscale binaries and `/usr/local/share/licenses/tailscale`.

   Result: Version/build evidence binds the complete patch hash. Source hash,
   exact patch, upstream/Go/consumed-module notices are retained. Go, source and
   caches are absent from the runtime images. No published pin is invented.

3. Compare binary hashes from two independent builds with identical inputs.

   Result: Claim reproducibility only on equal binary hashes; matching markers
   or a source/compiler pin is insufficient. Record a missing comparison pending.

### 6.4 U04 — Repository integration

1. Run affected worker Python, skills coverage, maintainability, documentation
   and candidate readiness checks under the repository input/reuse rules.

   Result: CPU/GPU contexts contain exactly the declared runtime inputs, exclude
   unknown/symlink inputs, and both bundled skills describe the same boundary.

### 6.5 U05 — Separate consumer acceptance gate

1. In the subsequent consumer outcome, test missing, extra, truncated and late
   SCM_RIGHTS, wrong/zero prefix, other control messages and wrong endpoint
   family/type/peer. Check original daemon/listener before and after receipt.

   Result: Bounded receive immediately owns every delivered FD. Refusal,
   timeout, unpolled drop, queued-rights/listener retirement and cancellation
   close all original/temporary descriptors. Header parsing consumes only its
   bounded header after `P`; application bytes and caller metadata grant no peer.

2. Exercise consumer finish with normal `0x01`+EOF, EOF alone, wrong/extra byte,
   byte without EOF, daemon death and cancel/deadline/phase races; cancel while
   finish retains a private reference and after payload write EOF.

   Result: Only exact ACK+EOF under existing deadlines permits completion.
   Whole cancel shuts original payload/control immediately despite retained
   references; ordinary write EOF preserves control. Budget grace/discard bounds
   remain unchanged. All original tasks/FDs retire before cleanup is reported.

3. Run independently approved first/final genuine original Kernel and userspace
   Worker consumer cases, with real payload/EOF, generic-proxy forgery refusal
   and exact original process/listener/config cleanup.

   Result: Both modes meet their actual source/custody contracts. Socketpair
   mechanics and runtime image success alone cannot supply AcceptedPeer authority.

## 7. Pass criteria

Runtime qualification requires U01–U04 on the current candidate on both native
architectures where applicable. Reproducibility stays pending until the independent
binary comparison passes. U05 is a separate
consumer gate, not satisfied by this runtime outcome. Record unrun descriptor,
image, reproducibility and actual-consumer checks pending; do not transfer older
non-descriptor green evidence. No result activates M2 or grants runtime identity.

## 8. Cleanup

1. Remove only this run's exact image tags, sockets and temporary directories.

   Result: Owned test processes/FDs/goroutines are retired; original workers,
   credentials, daemon sockets, published pins and shared caches remain intact.

## 9. Record of results

Record exact candidate/uncommitted inputs, commands, architecture, raw test names
and results, source/binary hashes, native image digests and owned cleanup. Keep
expected-red, current green, reused, pending and non-applicable results distinct.
Describe capability support without claiming remote application delivery or
completed Producer/Gate/Native qualification.
