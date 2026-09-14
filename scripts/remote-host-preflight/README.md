# Remote host preflight (Linux worker candidates)

Standalone, **read-only** capability checker for running a Horizon Linux
worker on an existing user-owned machine (first slice of
[issue #604](https://github.com/peters/horizon/issues/604)). It probes the
selected Linux host, reports what the worker contract needs, and separates
**supported** / **unsupported** / **unverified** results. It never installs,
pulls, runs or mutates anything.

## Usage

Run *on the target host* (SSH delivery/invocation is a later slice):

```sh
python3 -B scripts/remote-host-preflight/preflight.py              # human report
python3 -B scripts/remote-host-preflight/preflight.py --json       # machine report
```

Options:

- `--workspace-path PATH` — intended worker workspace directory
  (default `/var/lib/horizon-workers`). The storage qualifier is evaluated on
  the nearest existing ancestor when the path does not exist yet.
- `--timeout SECONDS` — per-probe timeout (default 10, maximum 3600).
- `--now ISO` — fixed `generated_at` timestamp for deterministic output.
- `--procfs-root` / `--sysfs-root` — alternate procfs/sysfs roots, for
  synthetic testing only.

Exit codes: `0` all decided prerequisites supported, `1` at least one
unsupported check **and no probe errors**, `2` at least one probe error
(errors take precedence; the report is still emitted).

## What it checks

| Check | Source | Meaning |
| --- | --- | --- |
| `os_linux` | `uname -srm` | Linux kernel on `x86_64` or `aarch64` (release + arch in the report). A failed or truncated `uname` is an **error**, not a rejection; only a parsed non-Linux kernel is `unsupported` |
| `container_engine` | `docker version --format json` / `docker context inspect --format '{{.Endpoints.docker.Host}}'` / `docker info --format '{{.Driver}}'` / `podman --remote=true --url unix://<existing-socket> info --format '{{.Version.Version}}'` | a usable engine is present **and reachable by the current user on this host**: the endpoint must be a local unix socket (`DOCKER_HOST` and the active docker context Host; named podman/`CONTAINER_*` connections are rejected). Podman socket discovery (`/run/podman/podman.sock` and `$XDG_RUNTIME_DIR/podman/podman.sock`) runs in a killable helper so a stale FUSE/NFS `XDG_RUNTIME_DIR` cannot block the main process. The engine is then queried only through that existing socket via `--remote=true --url unix://...` (never local `podman info`, which would initialize rootless runtime state). The docker server OS must be present and `linux`, and for docker the storage driver is reported or explicitly marked `unverified` with the reason. Engine probes request only those fields |
| `cpu_capacity` | `nproc`, fallback `/proc/cpuinfo` | at least the 4-core reference baseline. The cpuinfo fallback is used only when it contains at least one `processor` record; otherwise the report is incomplete |
| `memory_capacity` | `/proc/meminfo` `MemTotal` | at least the 16 GiB reference baseline minus 512 MiB (MemTotal excludes kernel-reserved pages; an unreadable `MemTotal` is an **error**, not a rejection) |
| `disk_capacity` | `df -kP PATH` | at least 20 GiB free on the mount that will hold the workspace. `PATH` is the nearest existing workspace ancestor (one extra argv element, no shell); unrelated mounts are not queried. The **longest mount-point ancestor** of the resolved path is selected; a malformed free value on that mount is an **error** |
| `storage_ext4_qualifier` | `stat` + `/sys/dev/block` + `/proc/fs/ext4/<dev>/options` | the worker repository-storage qualifier, mirrored exactly from `crates/horizon-core/src/repository_overlay/storage.rs`: ext4 options with exact tokens `rw` + `barrier`, no `ro`/`nobarrier`, exactly one `data=ordered`/`data=journal` line, no duplicates/spaces/control characters, 4096-byte cap, trailing newline |
| `tailscale` | `tailscale version` / `tailscale status --json --peers=false` | **informational, never gates the verdict** (issue #604 permits ordinary pinned SSH): presence + self DNS name + online state when available, `unverified` when absent. Peer inventory is not requested |
| `storage_durability`, `effective_isolation`, `worker_startup` | — | always **unverified**: read-only metadata cannot prove retention, isolation or startup; the later on-worker acceptance closes these |

## Read-only guarantees

- Every subprocess call is one of the fixed argv vectors in `PROBE_ARGS`
  (`shell=False`, per-probe timeout). The tests enforce this allowlist, so a
  regression that interpolates host values into probe arguments fails CI.
- Direct file reads are limited to fixed paths under `--procfs-root` /
  `--sysfs-root`.
- The report contains only fixed fields. Any host-provided text that is
  surfaced (error excerpts, daemon-provided version strings, Tailscale DNS
  names) passes through a credential redactor (JWT-like material,
  private-key blocks, `password|token|secret|api_key` assignments redacted
  to end of line including quoted JSON keys (`"password":`, `"Authorization":`),
  `Authorization` headers case-insensitively), truncated to
  400 characters, including prefixed assignment keys such as
  `access_token=` / `refresh-token=` / `client_secret=`. Only the
  endpoint-selection variables `DOCKER_HOST`, `DOCKER_CONTEXT`,
  `PODMAN_CONNECTION`, `PODMAN_HOST`, `CONTAINER_HOST`, and
  `CONTAINER_CONNECTION` (presence) are inspected, plus `XDG_RUNTIME_DIR`
  when locating a local Podman API socket, plus the active docker context
  Host from the formatted inspect
  probe — never an environment dump; full engine configuration, peer
  inventories and unrelated workloads are never read or reported. `--timeout`
  must be a positive finite number.

## Tests

```sh
python3 -B -m unittest discover -s scripts/remote-host-preflight/tests -v
```

Deterministic: synthetic procfs/sysfs roots plus an injected executor with
fixture outputs. No test invokes a real host tool; the executor additionally
asserts every issued argv is one of the fixed probe vectors.
