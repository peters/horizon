# 2026-09-13 — #604 remote-host-preflight: Fintermac live smoke (repeatable)

Temporary validation artifact for the `scripts/remote-host-preflight` slice of
[#604](https://github.com/peters/horizon/issues/604). Delete after the live
pass is complete.

## Target

Motivating example: a Linux VM hosted on a user-owned Mac, reachable over
SSH. **Do not commit real hostnames, DNS names, aliases or logins.** Supply
them locally:

```sh
MAC_SSH=user@mac-host    # ssh target for the Mac (alias or user@host)
VM_SSH=user@linux-vm     # ssh target for the Linux VM (pinned host key)
```

The **Linux VM** is the preflight target. The Mac is only used to discover
the VM when its address is not already known. Nothing here requires the Mac
itself as the probe target — only SSH reachability of the Linux VM.

## Preconditions

- [ ] The Mac answers: `ssh -o BatchMode=yes -o ConnectTimeout=15 "$MAC_SSH" 'echo MAC_REACHABLE'`
- [ ] Identify the VM: `ssh "$MAC_SSH" 'tailscale status 2>/dev/null || true'`
      → pick the **Linux** node. Record it as `VM_SSH` (`user@name` or
      `user@ip`). If the VM is not a tailnet node, get its reachable address
      from the Mac (pinned SSH, known host key).
- [ ] Key-based auth to the VM works non-interactively:
      `ssh -o BatchMode=yes -o ConnectTimeout=15 "$VM_SSH" 'echo VM_REACHABLE'`
      (keep key-based auth and pinned host-key checking; do not add
      `StrictHostKeyChecking=no`).

## Scope note: what is read-only here

The **tool** is read-only (proven in the PR: strace audit, no-write runs).
The *verification plumbing* below — delivering the single checker file to
`/tmp` on the VM and deleting it afterwards — deliberately is **not**
read-only, and it is **not** a tool feature: issue #604 reserves SSH
delivery/invocation for a later slice. The before/after snapshot in steps 1
and 4 evidences the *tool run* (step 3), not the plumbing. The plan is
explicit about which steps mutate the host so the proof is not overclaimed.

## Steps (run in order)

1. **Baseline snapshot (read-only):**
   ```sh
   ssh "$VM_SSH" 'date -u; uname -srm; nproc; grep MemTotal /proc/meminfo;
     WS=/var/lib/horizon-workers;
     while [ ! -e "$WS" ]; do
       parent=$(dirname "$WS");
       [ "$parent" = "$WS" ] && break;
       WS=$parent;
     done;
     WS=$(readlink -f "$WS" 2>/dev/null || echo "$WS");
     df -kP "$WS" | tail -1;
     docker info --format "{{.ServerVersion}}" 2>&1 | head -1 || true;
     stat -c "ws=%n dev=%Hd:%Ld" "$WS" 2>/dev/null || echo "ws missing: $WS";
     B=$(basename "$(readlink /sys/dev/block/$(stat -c "%Hd:%Ld" "$WS" 2>/dev/null) 2>/dev/null)" 2>/dev/null);
     [ -n "$B" ] && { echo "dev=$B"; head -c 4096 "/proc/fs/ext4/$B/options"; echo; } || echo "no ext4 options"'
   ```
   Save as `before.txt`. Resolve `WS` to the nearest existing workspace
   ancestor first, then `df` that path (not `/`), record GNU `stat` `%Hd:%Ld`
   (filesystem `st_dev` major/minor — not `%t:%T`/`st_rdev`) and the same
   4096-byte ext4 options window the checker evaluates. That is the
   independent ground truth for matrix assertions 4–6.

2. **Deliver the checker — MUTATES the host (verification plumbing, not a tool feature):**
   single file to `/tmp`, removed in step 5. No package manager, no service,
   no install path.
   ```sh
   scp scripts/remote-host-preflight/preflight.py "$VM_SSH":/tmp/preflight-604.py
   ```

3. **Run the preflight on the VM** (the tool run under test; fixed args, bounded probes, 10 s each). Capture reports **locally** before any VM cleanup. Assertion 9 needs two JSON runs with the same `--now`:
   ```sh
   ssh "$VM_SSH" 'python3 -B /tmp/preflight-604.py --json --now 2026-09-13T00:00:00Z' > preflight-1.json
   echo json_exit_1=$?
   ssh "$VM_SSH" 'python3 -B /tmp/preflight-604.py --json --now 2026-09-13T00:00:00Z' > preflight-2.json
   echo json_exit_2=$?
   cmp preflight-1.json preflight-2.json
   ssh "$VM_SSH" 'python3 -B /tmp/preflight-604.py' > preflight.txt
   echo human_exit=$?
   ```
   Keep `preflight-1.json`, `preflight-2.json` and `preflight.txt` as the
   evidence artifacts. Do not rely on a VM-side report file (step 5 deletes
   only the delivered checker). On a live host, `df` free space can drift
   between the two JSON runs; if `cmp` differs only in `disk_capacity`,
   treat that as sampling, not a generated_at/probe-structure failure.

4. **Post-run snapshot (read-only):** same commands as step 1 (including
   `nproc`, `MemTotal`, the workspace ancestor, its block device and the raw
   ext4 options) into
   `after.txt`. Take `after.txt` **before** step 5 so the before/after diff
   is clock-only for the tool run (the delivered `/tmp/preflight-604.py`
   existed in both snapshots; reports were captured locally in step 3).

   Read-only proof of the tool additionally comes from the strace audit in
   the PR description (4 writes total, all to stdout; zero file-creating
   opens) — the snapshot diff is corroborating, not the primary evidence.

5. **Remove the verification copy** (the delivered checker, not host state):
   ```sh
   ssh "$VM_SSH" 'rm -f /tmp/preflight-604.py'
   ```

## Bug-hunt matrix (assert each on the real output)

| # | Assertion |
|---|-----------|
| 1 | Exit code is 0/1/2 and matches the JSON `summary` counts exactly |
| 2 | `os_linux` value is the VM's real `uname -srm`; status matches the arch rules (x86_64/aarch64 → supported) |
| 3 | `container_engine` names the engine actually running (cross-check against step 1 `docker info` version); storage driver reported for docker |
| 4 | `cpu_capacity` / `memory_capacity` values match `nproc` / `MemTotal` read independently in step 1 |
| 5 | `disk_capacity` free space matches `df -kP` for the workspace path (or `/`) within sampling drift |
| 6 | `storage_ext4_qualifier` device name matches the real block device of the workspace filesystem (`stat -c '%Hd:%Ld'` → `/sys/dev/block/<maj>:<min>`); pass/fail against the full 4096-byte ext4 options captured independently |
| 7 | `tailscale` DNS name matches the VM's tailnet identity from precondition 2; online state is a bool |
| 8 | No raw secrets/tokens/keys anywhere in either report (visual + `grep -E 'eyJ[A-Za-z0-9_-]{4,}\.|PRIVATE KEY|token='`) |
| 9 | Two runs with the same `--now` produce byte-identical JSON (`cmp` in step 3); disk free-space sampling drift is the only allowed difference |
| 10 | The 3 `unverified` entries are always present with their fixed details |
| 11 | `before.txt`/`after.txt` diff is clock-only for the tool run (corroboration; primary read-only evidence is the strace audit in the PR) |
| 12 | If the VM has no docker: the engine check says `docker: tool not present` / podman path — no crash, exit code still consistent |

## Failure handling

- Any assertion failing → capture the exact report + command, file findings
  on the PR (fix in scope), rerun the failed lane after the fix.
- VM reachable but a probe hangs beyond `--timeout` → that is a finding
  (timeout must fire; per-probe default 10 s).
- Mac reachable but VM not found/not reachable → stop, report, ask the
  maintainer for the VM address (no guesswork, no new firewall holes).

## Out of scope (later slices, per #604)

SSH delivery/invocation from the client, host registration, worker
bootstrap/start, storage durability and isolation proof, reconnect
acceptance. This plan only runs the read-only preflight.
