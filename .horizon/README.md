# Cloud development

The `cpu` profile is the default for the standard validation matrix and software
rendering. The `gpu` profile requires a GPU and includes the CUDA 13 toolchain
for `speech-cuda` builds. It never falls back to a CPU allocation. Both profiles
include Rust 1.98.1, formatting/lint tools, speech and graphics headers, the
configured agent, both browser engines, and a private native desktop.

The `cpu` profile requests at least 4 vCPU and 8 GB memory for one task per worker.
The `gpu` profile requests at least 8 vCPU and 32 GB memory.
These values are allocation minimums. They do not establish that a worker completes
the full validation matrix. Record the worker resources, validation results and
memory measurements for each qualification run.

Use the [worker value procedure](../docs/testing/procedures/cloud-worker-value.md)
to compare qualified workers by total cost per successful task. The
[small CPU worker report](../docs/testing/reports/cpu-small-worker-qualification.md)
records a completed CPU matrix on one worker. The run used warm and mixed build
caches after an image repair. It does not establish the cheapest worker or an
untouched cold start of that image.

Both profiles enable only `claude`. The tested provider runtime denies the user
namespaces required by `codex` protected execution, so that agent is intentionally
excluded from the advertised capabilities even though its CLI remains installed
in the tooling image. Enable it only after normal protected execution passes on
a compatible runtime; broader agent qualification remains tracked in issue #813.

Horizon looks up the newest Codex and Claude Code releases before each image
build and passes them as `HORIZON_CODEX_VERSION` and `HORIZON_CLAUDE_VERSION`.
Each agent installs in its own layer after the toolchain layer, Claude last
because it changes most often, so a new release rebuilds only that agent's layer
and the steps after it. Each layer checks that the agent reports the release it
installed and records it in `/etc/horizon-worker/agent-versions.json`. Codex
installs only together with the corresponding sources derived from its release
tag, as described in [THIRD-PARTY.md](THIRD-PARTY.md). If a release no longer
has the source layout that derivation reads, the build prints a `WARNING` and
installs the last verified Codex, 0.155.1, instead; review the pin in
`retain-component-sources.py` then. A checksum mismatch or failed download stops
the build. Manual builds without these arguments install Codex 0.155.1 and
Claude Code 2.1.278, so they stay reproducible. After changing the derivation,
run `python3 -B .horizon/test_retain_component_sources.py`.

The public images target `ghcr.io/peters/horizon-development`. Configure provider
credentials, GPU selection and publishing credentials in machine-local settings.
Repository YAML cannot choose credentials. The recipes use only public upstream
bases and compile worker helpers from a pinned public Horizon revision; no
private registry access is needed to build them. The context contains only
recipes, the installer and notices. Runtime source, package caches, account
authentication and application state are transferred separately.

Both images use Ubuntu 24.04. Chromium comes from the signed Debian snapshot dated 2026-09-22, installed
with two uniquely named compatibility libraries and Ubuntu-resolved dependencies;
no foreign apt repository or core-library downgrade is used. Preserve the
component licenses described in [THIRD-PARTY.md](THIRD-PARTY.md). The GPU image
retains the complete upstream CUDA development base and its vendor notices.

Commit the intended source revision and hydrate its Git LFS assets before
deploying. New shell and agent panels share the cloud checkout; create branches
and additional worktrees manually when needed. Run `.horizon/validate.sh cpu`
or `.horizon/validate.sh gpu` in the checkout you are validating. Build caches are separated by
worktree and profile on the persistent worker volume. Tests run serially by
default to reduce timing and port-reuse interference; set `RUST_TEST_THREADS`
explicitly to choose another concurrency level. When invoked as root, test
processes drop filesystem access-override capabilities so permission-denial
checks exercise filesystem mode bits. Test subprocesses also ignore global Git
configuration so synthetic repositories use their own LFS storage and settings.
A successful CUDA build
does not establish inference accuracy or hardware graphics rendering; those
require the corresponding live workload and adapter evidence.

The validation helper defaults to two build jobs for CPU workers and eight for GPU
workers. Set `CARGO_BUILD_JOBS` to override either default. If a CPU run needs less
memory, use one build job:

```sh
CARGO_BUILD_JOBS=1 .horizon/validate.sh cpu
```

The helper runs each validation command in sequence. Keep one validation run active
per worker. Additional builds and browsers can increase memory use.

The GPU validation helper uses `cargo rustc --locked -p horizon-ui --bin horizon
--features speech-cuda -- -l nccl`. This links the NCCL library already supplied
by the GPU base, working around missing link metadata in the speech dependency.
It does not fix plain `cargo build --features speech-cuda` on that base.

For native smoke, freeze the candidate binary, launch with a private application
home and config, unset `HORIZON`, and use the task-owned worker display through a
loopback SSH forward and a native Device panel in the current client workspace.
Keep the regular desktop, screenshot, recording, resize, persistence and cleanup
gates from `AGENTS.md`. Never count software rendering as GPU rendering. Browser
interaction uses the public browser tools; native input uses the worker's
explicit device target. A connected but unpresented viewer blocks interactive
acceptance. Keep recordings and operational identifiers private.

Provider runtimes must support the selected agents' normal protections. A blocked
user namespace is a failed protected-agent lane; do not disable the sandbox or
grant extra container privilege. Optional account login and package credentials
remain private runtime bindings. Delete task-owned workers and temporary pull
bindings after qualification, and verify provider removal.
