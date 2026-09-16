# Browser packaging readiness

Recorded for [#324](https://github.com/peters/horizon/issues/324). This is
measurement and API documentation only. **No crate is published as part of
this issue.** A future crates.io release needs a separate explicit approval.

Re-check the invariants with `./scripts/check-browser-packaging.sh`. Refresh
the tables below on the machine and commit named in each caption; do not treat
them as performance guarantees.

## Public API and semver

| Crate | crates.io | Role |
| --- | --- | --- |
| `horizon-browser` | allow-listed (`publish = ["crates-io"]`) | Embeddable engine: process ownership, CDP/WebDriver/BiDi, frames, commands |
| `horizon-browser-protocol` | unpublished (`publish = false`) | Backend-neutral serialized values; re-exported by the engine |
| `horizon-browser-control` | unpublished | Reusable ownership, manifests, queues, and discovery |
| `horizon-browser-mcp` | unpublished | Stdio MCP adapter for live Horizon panels |
| `horizon-browser-cli` | unpublished | `horizon-browser` binary: plans, prompt jobs, standalone MCP |
| `horizon-core` / `horizon-ui` | unpublished | Horizon product, not the reusable engine |

The engine's public Rust API is every reachable `pub` item from
`crates/horizon-browser/src/lib.rs`:

- root types and constants defined there (`BrowserConfig`,
  `ActiveBackendCapabilities`, `DEFAULT_VIEWPORT`);
- the crate-root `pub use` re-exports (`start_session`, `BrowserSession`,
  `BrowserCommand`, `BrowserEvent`, `FrameSlot`, `VideoCaptureHandle`,
  coordination/audit/network types, and protocol types);
- public modules `cdp`, `frames`, `input`, `process`, and `session`, including
  the public items those modules re-export.

Agents must keep using MCP, not that Rust API. The CLI is a binary, not a
published library. Making a currently public module private is a breaking
change after crates.io publication.

Until a crates.io release exists, the workspace version (`0.2.7` at this
recording) is a Horizon-internal sync token, not a public stability promise.
After an authorized publish:

- `horizon-browser` follows Cargo semver for that public surface;
- protocol types re-exported by the engine are part of that surface;
- `horizon-browser-protocol` must be published first (or the engine must stop
  depending on it as a registry crate). Verified `cargo package -p
  horizon-browser` currently fails because the protocol crate is not on
  crates.io. `cargo package --no-verify` still produces a local archive for
  inspection.
- MCP, CLI, core, and UI have no crates.io semver.

Do not run `cargo publish` from packaging, CI, or this issue.

## Compile graph

### Current dependency boundary

Measured on Linux x86_64 on 2026-09-16 at `7af7d206` after CLI/MCP decoupling
for #693. Counts use `cargo tree --locked -p <package> -e normal --prefix
none --format '{p}'`, deduplicating package name/version pairs and excluding
the selected root package. Build and development edges are excluded.

| Package | Unique normal dependency packages |
| --- | ---: |
| `horizon-browser-protocol` | 40 |
| `horizon-browser` (default features) | 92 |
| `horizon-browser-control` | 101 |
| `horizon-browser-mcp` | 178 |
| `horizon-browser-cli` | 183 |

None of these graphs includes `horizon-core` or `horizon-ui`. CLI and MCP use
the reusable browser-control crate; Horizon supplies product integration.
Video encoding is optional in the engine and explicitly enabled by its
full-feature consumers. The historical timings and sizes below have not been
remeasured here and do not describe these dependency graphs.

### Historical baseline before decoupling

The following measurements were taken on Linux x86_64 on 2026-09-13 at `ee29a162` with cached Cargo sources
and a **separate empty target directory** for each `cargo check`. `%M` is GNU
`time` peak RSS in KiB.

| Package | Unique normal dependency packages | `cargo check` | Peak build RSS |
| --- | ---: | ---: | ---: |
| `horizon-browser-protocol` | 12 | 4.11 s | 290032 KiB |
| `horizon-browser` | 88 | 7.15 s | 384524 KiB |
| `horizon-browser-mcp` | 219 | — | — |
| `horizon-browser-cli` | 222 | — | — |

Compared with the 2026-08-29 protocol README baseline (14 packages / 3.57 s /
288552 KiB for protocol; 61 packages / 4.90 s / 354844 KiB for the engine),
the protocol graph stayed small. The engine graph grew with later capture
work (including WebM). At that baseline, MCP and CLI pulled in `horizon-core`;
those counts are superseded by the current graph above.

A protocol-only client still does not compile `tungstenite`, `png`,
`zune-jpeg`, `rmcp`, `tokio`, `horizon-core`, or `horizon-ui`.

## Release binary and process cost

Historical measurements at `ee29a162` on the same Linux x86_64 host, `cargo build --release -p
horizon-browser-cli --locked`, binary `target/release/horizon-browser`.

| Metric | Value |
| --- | --- |
| Size | 10656504 bytes (10.2 MiB) |
| `--help` wall time | median 5.05 ms (n=30, min 4.36, max 6.08) |
| `--help` peak RSS | 4.1–5.0 MiB |
| `run` of `browser_list` only (no live panel) | 0.12 s, 10120 KiB peak RSS |

`--help` is cold process start of the CLI, not Chromium/Firefox launch. The
list-only `run` starts the in-process MCP server and returns an empty panel
list under `HORIZON_BROWSER_ACTOR`. Workload-matched warm CPU/RSS for a
five-minute public WebSocket capture remains the performance-acceptance item
on #324, not this packaging record.

## Package inspection

At the historical `ee29a162` baseline:

- `cargo package -p horizon-browser-protocol --locked` succeeds. The archive
  is README, `Cargo.toml`, `Cargo.lock`, `src/*.rs`, plus Cargo-generated
  `Cargo.toml.orig` and `.cargo_vcs_info.json`. Use `cargo package --list`
  for the exact inventory.
- `cargo package -p horizon-browser --locked --no-verify` succeeds and does
  not include Horizon UI, MCP, CLI, core, or `scripts/browser-smoke`.
- `cargo doc -p horizon-browser --no-deps` succeeds.
- Verified `cargo package -p horizon-browser --locked` fails until
  `horizon-browser-protocol` exists on crates.io.

`./scripts/check-browser-packaging.sh` encodes those publish flags and
package-content invariants.
