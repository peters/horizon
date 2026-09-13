# 2026-09-13 — #604 remote-host-preflight: Fintermac live smoke (repeatable)

Temporary validation artifact for the `scripts/remote-host-preflight` slice of
[#604](https://github.com/peters/horizon/issues/604). Delete after the live
pass is complete.

## Target

- **Fintermac**: Linux VM hosted on `finter-sin-mac-studio-1` (macOS,
  Tailscale `finter-sin-mac-studio-1.tailnet-f382.ts.net`).
- SSH access alias `fintermac` (`~/.ssh/config`): user `fintermac`,
  HostName `finter-sin-mac-studio-1`. The **Linux VM** is the preflight
  target; its own Tailscale identity is discovered in step 2 (not known from
  this machine's tailnet list while the Mac is offline).
- The host was **offline (Mac asleep)** when this plan was written. The live
  run executes when the host is back; nothing here requires the Mac itself as
  the target — only SSH reachability of the Linux VM.

## Preconditions

- [ ] `ssh fintermac` (the Mac) answers: `ssh -o BatchMode=yes -o ConnectTimeout=15 fintermac 'echo MAC_REACHABLE'`
- [ ] Identify the VM: `ssh fintermac 'tailscale status 2>/dev/null || /Applications/Tailscale.app/Contents/MacOS/Tailscale status'`
      → pick the **Linux** node (name/IP). Record it as `$VM` (DNS name or
      tailnet IP). If the VM is not a tailnet node, get its reachable IP from
      the Mac and use it (pinned SSH, known host key — see step 3).
- [ ] Key-based auth to the VM works non-interactively:
      `ssh -o BatchMode=yes -o ConnectTimeout=15 fintermac@<VM> 'echo VM_REACHABLE'`
      (if the VM user differs, use that user; keep the key-based auth and
      pinned host key — do not weaken host-key checking.)

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
   ssh fintermac@<VM> 'date -u; uname -srm; nproc; grep MemTotal /proc/meminfo;
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
   scp scripts/remote-host-preflight/preflight.py fintermac@<VM>:/tmp/preflight-604.py
   ```

3. **Run the preflight on the VM** (the tool run under test; fixed args, bounded probes, 10 s each). Capture both reports **locally** before any VM cleanup:
   ```sh
   ssh fintermac@<VM> 'python3 -B /tmp/preflight-604.py --json --now 2026-09-13T00:00:00Z' > fintermac-preflight.json
   echo json_exit=$?
   ssh fintermac@<VM> 'python3 -B /tmp/preflight-604.py' > fintermac-preflight.txt
   echo human_exit=$?
   ```
   Keep `fintermac-preflight.json` and `fintermac-preflight.txt` as the
   evidence artifacts. Do not rely on a VM-side report file (step 5 deletes
   only the delivered checker).

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
   ssh fintermac@<VM> 'rm -f /tmp/preflight-604.py'
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
| 9 | Two runs with the same `--now` produce byte-identical JSON |
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
