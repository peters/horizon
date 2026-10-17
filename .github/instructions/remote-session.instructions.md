---
applyTo: "crates/remote-session/**"
---

# Remote session boundary

Read `docs/architecture/remote-session.md` for the package boundary and current scope.

- The caller supplies the destination, credentials, runtime and exact host key pin.
- A server must pass host verification before authentication material is sent.
- Refuse unknown or changed keys. Do not add automatic trust or configuration discovery.
- Keep stdout, stderr, EOF and exit status distinct.
- Bound retained data and pending work. A stalled consumer must not prevent cancellation or the deadline.
- Stop and join owned transport work on explicit shutdown. Do not retry uncertain commands.
- Keep key material and server payloads out of debug output and errors.
- Do not infer Horizon session authorization from successful SSH authentication.
- This outcome does not replace Horizon's existing SSH paths or implement Mosh.
