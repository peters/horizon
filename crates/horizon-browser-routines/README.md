# Horizon Browser Routines

`horizon-browser-routines` holds the backend-neutral Teach-mode recording
protocol, the deterministic draft-plan compiler, the in-memory Teach session
(start/pause/stop/discard, with private drafts), the private routine registry, and the credential-broker
interface (fake store in this crate; OS adapters land later). It depends on `horizon-browser-protocol` for the shared action
model and on nothing in `horizon-browser`, the MCP adapter, the CLI runner, or
the UI.

This crate is an internal workspace package and is not published.

See `docs/architecture/browser-routines.md` and
`docs/architecture/browser-routine-credentials.md`.
