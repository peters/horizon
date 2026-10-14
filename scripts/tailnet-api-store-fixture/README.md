# Synthetic tailnet API store fixture

This is the reproducible **mandatory F05** lane for the API storage foundation.
It uses the real Linux Secret Service adapter with synthetic values only. It does
not start Horizon's UI, enroll a node, call Tailscale's API or provision a worker.
The optional authorized desktop staged-resolver check, R01, remains separate and
private; this script does not read user credentials.

Read the [credential-store procedure](../../docs/testing/procedures/tailnet-api-credential-store.md#65-f05--isolated-real-os-store-adapter)
for expected phase results, evidence, failure handling and cleanup.

Requirements are Linux x86_64, Python 3, a healthy local Docker daemon, a hydrated
candidate checkout and the repository Rust toolchain. The host needs executable
`/usr/bin/dbus-daemon`, `/usr/bin/gnome-keyring-daemon`, `/usr/bin/gdbus` and runtime
libraries at `/usr/lib/x86_64-linux-gnu`. Typical Debian/Ubuntu packages are
`dbus`, `gnome-keyring` and `libglib2.0-bin`; use the packages for the host distro.
Get the exact runtime image before the run:

```sh
docker pull ghcr.io/peters/horizon-worker-base@sha256:6550aacb6c60187cd346a795beb3c8bffd998fe9b95ec74f5fb844cf78870bdc
```

That older image is only an OS runtime. It supplies no candidate-image, worker
contract, enrollment, tailnet or provider qualification.

From the candidate checkout root, choose a new private evidence directory.
Replace the example path below with a new path for this run:

```sh
python3 scripts/tailnet-api-store-fixture/run.py --evidence-dir /tmp/horizon-tailnet-api-f05-first
```

An optional `--target-dir /tmp/horizon-tailnet-api-f05-target` selects a separate build
cache. The runner automatically builds and freezes the library test executable:

```sh
cargo test -p horizon-cloud --lib --no-run --locked --message-format=json
```

The Linux fixture test is ignored in ordinary matrix runs because it requires an
isolated real OS store. The runner explicitly executes that ignored test in five
phases: `exercise`, `probe`, `unavailable`, `probe`, `cleanup`. An ignored/skipped
matrix test is **not** an F05 pass. No private external helper is required.

The fixture container has no external network, a read-only root, no capabilities
and `no-new-privileges`. Its home, runtime, state, bus, keyring control directory
and unlocked collection belong to this run. No host home, desktop bus, runtime
socket or graphical session is mounted. Read-only executable, script and OS tool
mounts provide the declared runtime. The custom bus has no activation directories,
so stopping the retained keyring cannot silently start a replacement.

Evidence records source and frozen artifact identities, phase results and exact
ownership privately. Success requires an empty owned Secret Service, stopped
owned processes and proof that the exact container is absent from a healthy Docker
daemon. The runner removes synthetic state and its password only after container
absence is proved; this cleanup also runs after a failed phase. A failed phase
is not an F05 pass. Logs and ownership records remain private. An unavailable
daemon or uncertain cleanup is not absence proof; preserve the ownership record
and remaining state for exact reconciliation. Never use Docker
prune, a process-name kill or the operator's bus as a fallback.

Run this lane for the first reviewable and final source. Reuse requires an exact
input comparison under AGENTS.md; an old artifact or another checkout's helper
result does not automatically qualify the candidate.
