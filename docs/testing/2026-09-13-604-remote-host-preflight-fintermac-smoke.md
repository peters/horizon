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

## Steps (run in order; the run is read-only by construction)

1. **Baseline snapshot (read-only):**
   ```sh
   ssh fintermac@<VM> 'date -u; uname -srm; df -kP / | tail -1; docker info --format "{{.ServerVersion}}" 2>&1 | head -1 || true'
   ```
   Save as `before.txt`.

2. **Deliver the checker** (single file, to `/tmp` — no install):
   ```sh
   scp scripts/remote-host-preflight/preflight.py fintermac@<VM>:/tmp/preflight-604.py
   ```

3. **Run the preflight on the VM** (fixed args, bounded probes, 10 s each):
   ```sh
   ssh fintermac@<VM> 'python3 -B /tmp/preflight-604.py --json --now 2026-09-13T00:00:00Z > /tmp/preflight-604.json; echo exit=$?; python3 -B /tmp/preflight-604.py'
   ```
   Save the JSON report as `fintermac-preflight.json` and the human report as
   `fintermac-preflight.txt`.

4. **Post-run snapshot (read-only):** same commands as step 1 into
   `after.txt`; `diff before.txt after.txt` must show only the clock line
   changed. This is the read-only proof (the tool also never writes — see the
   strace audit in the PR description).

5. **Remove the delivery copy** (our own `/tmp` file, not host state):
   ```sh
   ssh fintermac@<VM> 'rm -f /tmp/preflight-604.py /tmp/preflight-604.json'
   ```

## Bug-hunt matrix (assert each on the real output)

| # | Assertion |
|---|-----------|
| 1 | Exit code is 0/1/2 and matches the JSON `summary` counts exactly |
| 2 | `os_linux` value is the VM's real `uname -srm`; status matches the arch rules (x86_64/aarch64 → supported) |
| 3 | `container_engine` names the engine actually running (cross-check against step 1 `docker info` version); storage driver reported for docker |
| 4 | `cpu_capacity` / `memory_capacity` values match `nproc` / `MemTotal` read independently in step 1 |
| 5 | `disk_capacity` free space matches `df -kP` for the workspace path (or `/`) within sampling drift |
| 6 | `storage_ext4_qualifier` device name matches the real block device of the workspace filesystem (`stat -c '%t:%T'`); pass/fail against the ext4 options listed independently (`cat /proc/fs/ext4/<dev>/options`) |
| 7 | `tailscale` DNS name matches the VM's tailnet identity from precondition 2; online state is a bool |
| 8 | No raw secrets/tokens/keys anywhere in either report (visual + `grep -E 'eyJ[A-Za-z0-9_-]{4,}\.|PRIVATE KEY|token='`) |
| 9 | Two runs with the same `--now` produce byte-identical JSON |
| 10 | The 3 `unverified` entries are always present with their fixed details |
| 11 | `before.txt`/`after.txt` diff is clock-only (read-only proof) |
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
