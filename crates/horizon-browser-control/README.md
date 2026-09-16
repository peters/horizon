# horizon-browser-control

Shared filesystem coordination for browser hosts and clients. This crate owns
live manifests, stable file locks, ownership leases, handoff, bounded action and
host-request queues, result files, and redacted audit journals. It implements
the engine's `BrowserCoordination` trait without depending on `horizon-core`,
terminal state, a UI toolkit, MCP, or an agent runtime.

The initial extraction preserves the existing `HOME/.horizon` default, including
the relative `.horizon` fallback when `HOME` is unset. `BrowserRuntimePaths`
provides path construction; APIs with explicit root/path arguments retain their
existing behavior. Constructing a path object does not change the default root
used by `ManifestCoordination` or free functions.

`horizon-core::browser::manifest` reexports this implementation. Both entrypoints
therefore share the same process host identity, ownership rules, file format,
locking protocol, and audit redaction. Browser providers, credentials, device
quotas, Teach state, and panel/workspace models remain with their existing hosts.

```sh
cargo test -p horizon-browser-control
```

This crate is not published. The standalone CLI and MCP server consume it
directly without depending on Horizon core.

## Runtime paths

Hosts can call `paths::configure_runtime_root(path)` before using default-path
coordination helpers. Configuration resolves the path to an absolute path and
freezes it for the process. Repeating that absolute root is allowed; a different
root or configuration after default-path use returns a typed error. Explicit
`*_at` APIs and `BrowserRuntimePaths::from_root` remain available for independent
instances.

The standalone CLI and MCP server initialize from `HORIZON_BROWSER_ROOT` before
discovery, pruning or serving. Relative values resolve against the startup
directory; empty values are rejected. Without the variable they retain the
existing `HOME/.horizon` default, including the relative fallback when HOME is
absent. Library hosts that use legacy default helpers without initialization
retain the existing default behavior. Application config
and terminal sessions continue using the application home independently.

Task-owned browser subprocesses and agent MCP registrations explicitly select
their private browser root, so an inherited override cannot redirect job control
into an unrelated runtime. The variable does not change provider credentials or
authorization rules.
