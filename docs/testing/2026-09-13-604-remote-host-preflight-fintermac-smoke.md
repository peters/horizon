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

The **tool** is read-only (fixed argv allowlist, no write/install/pull APIs).
The *verification plumbing* below — delivering the single checker file to
`/tmp` on the VM and deleting it afterwards — deliberately is **not**
read-only, and it is **not** a tool feature: issue #604 reserves SSH
delivery/invocation for a later slice. The before/after snapshot in steps 2
and 4 evidences the *tool run* (step 3), not the plumbing. Delivery is step 1
so both snapshots include the checker file. The plan is
explicit about which steps mutate the host so the proof is not overclaimed.

## Steps (run in order)

1. **Deliver the checker — MUTATES the host (verification plumbing, not a tool feature):**
   unique remote directory from `mktemp -d`, removed in step 5. Copy
   `preflight.py` and sibling `executor.py` there and invoke from that
   directory so `from executor import` resolves. Do not use shared
   `/tmp/preflight-604.py` names. No package manager, no service,
   no install path. Deliver **before** the baseline so both snapshots include
   the files and the delivery itself is not part of the before/after diff.
   ```sh
   REMOTE_DIR_RAW=$(ssh "$VM_SSH" 'mktemp -d /tmp/preflight-604.XXXXXX') || exit 1
   REMOTE_DIR=$(printf '%s' "$REMOTE_DIR_RAW" | tr -d '\r')
   case "$REMOTE_DIR" in
     /tmp/preflight-604.*) ;;
     *) echo "unexpected remote dir: $REMOTE_DIR" >&2; exit 1 ;;
   esac
   scp scripts/remote-host-preflight/preflight.py "$VM_SSH":"$REMOTE_DIR/preflight.py"
   scp scripts/remote-host-preflight/executor.py "$VM_SSH":"$REMOTE_DIR/executor.py"
   ```

2. **Baseline snapshot (read-only):** write the independent ground truth
   to `before.txt` via the redirection below (assertion 11 diffs this file
   against `after.txt`). Canonicalize `WS` with `readlink -m` **before** the
   ancestor walk (so a dangling workspace symlink is judged on the target
   side, matching the checker), then `df` that path (not `/`), record GNU `stat` `%Hd:%Ld`
   (filesystem `st_dev` major/minor — not `%t:%T`/`st_rdev`) and the same
   4097-byte ext4 options window the checker evaluates (4096+1 to detect
   overflow, no extra newline). That is the
   independent ground truth for matrix assertions 4–6.
   ```sh
   SNAPSHOT_REMOTE='date -u; uname -srm; nproc; grep MemTotal /proc/meminfo;
     WS=/var/lib/horizon-workers;
     WS=$(readlink -m "$WS" 2>/dev/null || echo "$WS");
     while [ ! -e "$WS" ]; do
       parent=$(dirname "$WS");
       [ "$parent" = "$WS" ] && break;
       WS=$parent;
     done;
     df -kP "$WS" | tail -1;
     if command -v docker >/dev/null 2>&1; then
       if [ -n "${DOCKER_CONTEXT:-}" ]; then
         HOST=$(docker context inspect --format '{{.Endpoints.docker.Host}}' 2>/dev/null);
       elif [ -n "${DOCKER_HOST:-}" ]; then
         HOST="$DOCKER_HOST";
       else
         HOST=$(docker context inspect --format '{{.Endpoints.docker.Host}}' 2>/dev/null);
       fi;
       case "$HOST" in
         unix://*|/*) docker --host "$HOST" info --format "{{.ServerVersion}}" 2>&1 | head -1 ;;
         *) echo "docker endpoint is not a local unix socket" ;;
       esac
     else
       echo "docker: tool not present";
     fi;
     PODMAN_VER="";
     CAND="";
     [ -n "${XDG_RUNTIME_DIR:-}" ] && CAND="$XDG_RUNTIME_DIR/podman/podman.sock";
     UID_SOCK="/run/user/$(id -u)/podman/podman.sock";
     for SOCK in $CAND $UID_SOCK /run/podman/podman.sock; do
       [ -n "$SOCK" ] && [ -S "$SOCK" ] || continue;
       VER=$(podman --remote=true --url "unix://$SOCK" info --format "{{.Version.Version}}" 2>/dev/null | head -1);
       if [ -n "$VER" ]; then PODMAN_VER=$VER; break; fi;
     done;
     if [ -n "$PODMAN_VER" ]; then
       echo "$PODMAN_VER";
     else
       echo "podman local service is not running";
     fi;
     stat -c "ws=%n dev=%Hd:%Ld" "$WS" 2>/dev/null || echo "ws missing: $WS";
     B=$(basename "$(readlink /sys/dev/block/$(stat -c "%Hd:%Ld" "$WS" 2>/dev/null) 2>/dev/null)" 2>/dev/null);
     [ -n "$B" ] && { echo "dev=$B"; head -c 4097 "/proc/fs/ext4/$B/options"; } || echo "no ext4 options"'
   ssh "$VM_SSH" "$SNAPSHOT_REMOTE" > before.txt
   ```

3. **Run the preflight on the VM** (the tool run under test; fixed args, bounded probes, 10 s each). Capture reports **locally** before any VM cleanup. Assertion 9 needs two JSON runs with the same `--now`:
   ```sh
   ssh "$VM_SSH" "python3 -B '$REMOTE_DIR/preflight.py' --json --now 2026-09-13T00:00:00Z" > preflight-1.json
   echo json_exit_1=$?
   ssh "$VM_SSH" "python3 -B '$REMOTE_DIR/preflight.py' --json --now 2026-09-13T00:00:00Z" > preflight-2.json
   echo json_exit_2=$?
   cmp preflight-1.json preflight-2.json
   ssh "$VM_SSH" "python3 -B '$REMOTE_DIR/preflight.py'" > preflight.txt
   echo human_exit=$?
   ```
   Keep `preflight-1.json`, `preflight-2.json` and `preflight.txt` as the
   evidence artifacts. Do not rely on a VM-side report file (step 5 deletes
   only the delivered checker). On a live host, `df` free space can drift
   between the two JSON runs; if `cmp` differs only in `disk_capacity`,
   treat that as sampling, not a generated_at/probe-structure failure.

4. **Post-run snapshot (read-only):** reuse `SNAPSHOT_REMOTE` from step 2
   and write `after.txt` **before** step 5. The delivered checker files under
   `$REMOTE_DIR` existed in both snapshots; reports were captured locally in
   step 3.
   Compare invariant fields (`uname`, `nproc`, `MemTotal`, engine identity,
   workspace device, ext4 options). Allow `date -u` to change and allow
   `df` Available/Capacity sampling drift — inspect that drift the same way
   as the two JSON runs in step 3; do not treat a free-space change as a
   tool write. The checker's read-only contract is the fixed argv allowlist
   and the absence of write/install/pull APIs — not a partial syscall filter
   (`strace -e write,openat` omits `mkdir`/`unlink`/`rename`/`truncate` and
   is not claimed as proof here).
   ```sh
   ssh "$VM_SSH" "$SNAPSHOT_REMOTE" > after.txt
   ```

5. **Remove the verification copy** (the delivered checker, not host state):
   ```sh
   ssh "$VM_SSH" "rm -rf '$REMOTE_DIR'"
   ```

## Bug-hunt matrix (assert each on the real output)

| # | Assertion |
|---|-----------|
| 1 | Exit code is 0/1/2 and matches the JSON `summary` counts exactly |
| 2 | `os_linux` value is the VM's real `uname -srm`; status matches the arch rules (x86_64/aarch64 → supported) |
| 3 | `container_engine` names the engine actually running (cross-check against the step 2 docker and/or Podman remote-socket version capture; if only Podman is present, use that capture, not `docker info`); storage driver reported for docker |
| 4 | `cpu_capacity` / `memory_capacity` values match `nproc` / `MemTotal` read independently in step 2 |
| 5 | `disk_capacity` free space matches `df -kP` for the workspace path (or `/`) within sampling drift |
| 6 | `storage_ext4_qualifier` device name matches the real block device of the workspace filesystem (`stat -c '%Hd:%Ld'` → `/sys/dev/block/<maj>:<min>`); pass/fail against the full 4096-byte ext4 options captured independently |
| 7 | If Tailscale is present: DNS name matches the VM's tailnet identity from precondition 2 and online state is a bool. If Tailscale is absent (pinned SSH only): status is `unverified` and the verdict is not gated on it |
| 8 | No raw secrets/tokens/keys anywhere in either report (visual + `grep -E 'eyJ[A-Za-z0-9_-]{4,}\.|PRIVATE KEY|token='`) |
| 9 | Two runs with the same `--now` produce byte-identical JSON (`cmp` in step 3); disk free-space sampling drift is the only allowed difference |
| 10 | The 3 `unverified` entries are always present with their fixed details |
| 11 | `before.txt`/`after.txt` exist as files from the redirections in steps 2 and 4; the diff is limited to the clock and optional `df` free-space sampling drift (inspect that drift; live-run corroboration; the checker's read-only contract is the argv allowlist, not a partial syscall trace) |
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
