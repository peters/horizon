# Horizon Browser Routines

`horizon-browser-routines` holds the backend-neutral Teach-mode recording
protocol and the deterministic draft-plan compiler. The routine registry and
credential-broker interface land in later slices. It depends on
`horizon-browser-protocol` for the shared action model and on nothing in
`horizon-browser`, the MCP adapter, the CLI runner, or the UI.

This crate is an internal workspace package and is not published.

See `docs/architecture/browser-routines.md` and
`docs/architecture/browser-routine-credentials.md`.
