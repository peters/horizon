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
- `--timeout SECONDS` — per-probe timeout (default 10).
- `--now ISO` — fixed `generated_at` timestamp for deterministic output.
- `--procfs-root` / `--sysfs-root` — alternate procfs/sysfs roots, for
  synthetic testing only.

Exit codes: `0` all decided prerequisites supported, `1` at least one
unsupported, `2` at least one probe error (report is still emitted).

## What it checks

| Check | Source | Meaning |
| --- | --- | --- |
| `os_linux` | `uname -srm` | Linux kernel on `x86_64` or `aarch64` |
| `container_engine` | `docker version` / `docker info` / `podman info` | a usable engine is present **and reachable** by the current user; reports the engine version and (for docker) the storage driver |
| `cpu_capacity` | `nproc`, fallback `/proc/cpuinfo` | at least the 4-core reference baseline |
| `memory_capacity` | `/proc/meminfo` `MemTotal` | at least the 16 GiB reference baseline |
| `disk_capacity` | `df -kP` | at least 20 GiB free on the workspace filesystem (falls back to `/`) |
| `storage_ext4_qualifier` | `stat` + `/sys/dev/block` + `/proc/fs/ext4/<dev>/options` | the worker repository-storage qualifier: ext4 mounted with `rw`, `barrier`, exactly one `data=ordered`/`data=journal`, no `ro`/`nobarrier` (see `docs/testing/azure-workspace-readiness.md`) |
| `tailscale` | `tailscale version` / `tailscale status --json` | informational: presence, self DNS name, online state |
| `storage_durability`, `effective_isolation`, `worker_startup` | — | always **unverified**: read-only metadata cannot prove retention, isolation or startup; the later on-worker acceptance closes these |

## Read-only guarantees

- Every subprocess call is one of the fixed argv vectors in `PROBE_ARGS`
  (`shell=False`, per-probe timeout). The tests enforce this allowlist, so a
  regression that interpolates host values into probe arguments fails CI.
- Direct file reads are limited to fixed paths under `--procfs-root` /
  `--sysfs-root`.
- The report contains only fixed fields. Any host-provided text that is
  surfaced (error excerpts) passes through a credential redactor
  (JWT-like material, private-key blocks, `password|token|secret|api_key`
  assignments, `Authorization` headers), truncated to 400 characters.
  Environment variables, full engine configuration and unrelated workloads
  are never read or reported.

## Tests

```sh
python3 -B -m unittest discover -s scripts/remote-host-preflight/tests -v
```

Deterministic: synthetic procfs/sysfs roots plus an injected executor with
fixture outputs. No test invokes a real host tool; the executor additionally
asserts every issued argv is one of the fixed probe vectors.
