---
applyTo: "crates/horizon-cloud/src/tailnet/api.rs,crates/horizon-cloud/src/tailnet/api/credential.rs,crates/horizon-cloud/src/tailnet/api/node*,crates/horizon-cloud/src/tailnet/api/node/**"
---

# Desktop recorded-node HTTP boundary

Read docs/architecture/tailnet-node-api.md. This SDK client is unwired; do not attribute owner authentication, durable confirmation or automatic cloud deletion to it. The trusted caller must supply only captured IDs after provider absence. Preserve exact saved-ID managed/tombstone/staged resolver rules and auth/API independence, no public secret getter or new MCP mutation. Check fixed HTTPS origin, no proxy/redirect, bounded headers/body/time, one encoded opaque device segment, OAuth scope/tag subset and value-free typed errors. Missing/offline/denied/404 uncertainty must remain Retained. Production origin override is test-only. Offline fixtures do not establish real API authority.
