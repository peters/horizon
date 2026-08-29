# Horizon Browser Protocol

`horizon-browser-protocol` contains the serialized, backend-neutral values
shared by the browser engine, its MCP adapter, and lightweight clients. It has
no browser process, socket, async runtime, image decoder, filesystem
coordination, MCP, or Horizon UI dependency.

Use this crate when an application only needs to construct or inspect browser
actions, results, network records, and redacted audit entries. Use
`horizon-browser` when the application must launch and own Chromium, Firefox,
or Safari itself.

The crate is an internal workspace package for now and is not published.

## Build footprint baseline

Measured on Linux on 2026-08-29 with cached Cargo sources and a separate empty
target directory for each command:

| Package | Normal dependency packages | `cargo check` | Peak build RSS |
| --- | ---: | ---: | ---: |
| `horizon-browser-protocol` | 14 | 3.57 s | 288552 KiB |
| `horizon-browser` | 61 | 4.90 s | 354844 KiB |

The timings are machine-specific rather than performance guarantees. The
dependency boundary is the durable result: a protocol-only client does not
compile `tungstenite`, `png`, `zune-jpeg`, `rmcp`, `tokio`, `horizon-core`, or
`horizon-ui`. Applications that already compile Serde will normally reuse most
of the protocol crate's build graph.
