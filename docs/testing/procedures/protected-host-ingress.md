---
procedure: protected-host-ingress
feature: Opt-in new Hetzner host publication and original receipt producer
platforms: [linux]
cost: paid VM for H03 only, separately authorized capped run
destructive: owned VM, container and generation directory only
secrets: existing Horizon credentials remain in their original bindings
owner: peters
---

# Protected host ingress test procedure

## 1. Purpose

Test the optional host bootstrap and its actual original Docker readback.
Preserve the default host configuration and prove that an unprivileged agent
cannot take either protected physical pool after a root listener closes.
This does not qualify production Gate, keeper or Core exposure consumption.

## 2. Applicability

- H01/H02: the recorded source candidate and locked toolchain, no provider use.
- H03: one explicitly authorized, task-owned Hetzner `docker-ce` VM created by
  the normal cloud provisioning path with `bootstrap.exposure_ingress: true`.
- Run H03 on the first reviewable candidate and again after final behavior changes.
- No production tailnet, foreign worker, copied Ready state or credential copy.

## 3. Safety

Never run generated SETUP, GUARD, START or the production ingress script on the
desktop host. Those commands mask host SSH and manage a container. Real startup
belongs only in the owned VM. Do not mount a desktop Docker socket or host proc
into a worker. Do not change existing clouds or use a mutable worker image tag.
Use the authorized resource/cost cap and record exact cleanup after H03.

## 4. Equipment and preconditions

- Read `docs/architecture/protected-host-ingress.md` and the canonical Protocol
  `docs/architecture/exposure-protected-ingress.md` boundary.
- Have the original provider/client and cloud Store journal, pinned worker SSH,
  immutable worker image, and a separate original external client ready for H03.
- Verify Python3, Docker, nsenter and cat in the VM meet the fixed root-owned
  executable contract. Existing host bootstrap guards must remain enabled.
- Before provisioning, freeze candidate source, toolchain, normal executable,
  profile, immutable image and embedded producer-script hashes. The normal
  journal allocates the volume before generating user data, which contains its
  actual device path. This procedure does not capture the sent user-data bytes
  or their hash. Never present a reconstruction as wire evidence, or include
  credential payloads or full Docker inspection output.

## 5. Setup

1. Use an isolated task checkout and cache. Keep the default flag false.

   Result: Existing workspaces and the user's active processes remain intact.

2. Use only synthetic records for H01/H02. Do not invoke production `main`.

   Result: No Docker daemon, provider or SSH action occurs in the pure tests.

## 6. Tasks

### 6.1 H01 — Preserve defaults and decode the actual encoder output

1. Run `cargo test -p horizon-cloud --locked --offline` and the Core replacement
   compatibility and registry host-login regressions on the recorded graph.

   Result: False/absent intent preserves old serialization and host configuration.
   True intent is Hetzner-only, cannot be enabled or removed by rebuild, preserves
   existing guard/unit/SSH22/registry/volume behavior and never fabricates mappings.

2. Run `cargo test -p horizon-cloud-protocol --features exposure-gate --locked --offline`.

   Result: The exact production encoder with synthetic Docker observations emits
   bytes accepted by the canonical receipt parser and fixed port helpers. Host
   preflight, create/start and process observation are not called by this test.

### 6.2 H02 — Refuse unsafe observations and bound original child handling

1. Run `python3 -B -m unittest discover -s crates/horizon-cloud/src/host -p test_ingress.py`.

   Result: Complete actual publication sets are required. Extra, partial,
   duplicate, backend and non-TCP publications are refused. A generation cannot
   be overwritten. Failed readback cannot block exact known-CID cleanup; a lost
   create reply cannot select an unverified successor. Actual local pipe children
   are killed and reaped on overflow or timeout. No Docker command executes.

2. Complete the applicable repository local matrix on stable inputs.

   Result: Required tiers pass, advisory findings stay separate, and fresh normal
   UI graphs embed both current bundled references. This is local component proof.

### 6.3 H03 — Observe an actual owned VM and kernel protection

> **CAUTION:** This step rents paid capacity. Use only the declared task-owned VM,
> original account binding and approved cost cap. Preserve foreign resources.

1. Provision the declared VM through its normal original journal and immutable
   image with the explicit flag. Let cloud-init run the generated startup.

   Result: Existing guards run, only SSH22 and the fixed 32 ingress mappings are
   published, and no backend is published. Normal workspace Ready does not by
   itself establish protected exposure readiness.

2. Through original pinned worker-root SSH, read only the fixed receipt and
   independently observe actual PID1 start, own netns device/inode, effective
   sysctl, root capability and readonly root-owned file/mount metadata.

   Result: All observations match the original receipt. The file is mode0400,
   nlink1 and stable within its generation. No saved JSON manufactures authority.

3. Run task-owned root socket equipment on physical600/631 and632/663. Use the
   separate original external client to send bounded nonce bytes through actual
   external20000/20031 and record the accepted peer addresses.

   Result: Public forwarding reaches the correct physical listeners; backend
   ports are not publicly reachable. Claim source preservation only for actually
   checked families and paths. An IPv6-to-IPv4 proxy is not original peer proof.

4. Before, during and after root-listener shutdown, try explicit binds across both
   physical pools using UID10001 with the exact existing `setpriv` empty bounding
   set, clear-groups and no-new-privileges pattern. Then try root binds.

   Result: Agents receive permission denial after listeners close; root can bind.
   Static Docker publication cannot let an agent replace a closed Gate/backend.

> **CAUTION:** This step stops and deletes task-owned resources. Match the original
> journal, allocation, CID and generation; never act on a replacement or foreign ID.

5. Stop the task worker using its normal owned lifecycle; observe exact container
   and owned resource cleanup. Preserve failures and uncertain absence honestly.

   Result: Receipt removal precedes waits; only the original CID/generation is
   cleaned. No replaced container, JSON record or failed query supplies absence.

## 7. Pass criteria

- H01/H02 and required local checks pass on exact current inputs.
- H03 supplies actual Docker/kernel/source observations on the first and final
  reviewable behavior. Otherwise the producer remains unqualified for publication.
- Component results never claim the full epic, public Ready, Gate closure or TLS.

## 8. Cleanup

1. Delete only the original task-owned provider resources through their retained
   lifecycle and verify provider absence. Preserve original network devices.

   Result: No billed VM, volume, foreign mount or worker process remains owned by
   this run; uncertain operations retain their original records and bindings.

## 9. Record of results

Keep exact source, normal executable, profile, image and producer-script identity,
actual allocation/spec/volume and receipt/kernel observations, commands, bounded
observation logs, first/final smoke evidence, costs and cleanup in the task evidence. Record new,
reused and non-applicable checks separately. Follow `docs/testing/reports/TEMPLATE.md`
when an actual live run requires a permanent report.
