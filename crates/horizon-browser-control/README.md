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

This crate is not published. Configurable coordination roots and direct CLI/MCP
consumption are separate follow-up steps in issue #693.
