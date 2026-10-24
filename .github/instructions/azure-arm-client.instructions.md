---
applyTo: "crates/horizon-cloud/src/azure.rs,crates/horizon-cloud/src/azure/**"
---

# Azure ARM client boundary

Read `docs/architecture/azure-arm-client.md` before review.

- An ARM answer is untrusted data: status, headers and body.
- Build each URL only from validated segments. A bad segment sends nothing.
- Keep the HTTPS-only, no-redirect and 30-second rules of the production agent.
  Do not make `with_agent` public.
- Read a success body with `RESPONSE_LIMIT` and a failure body with
  `FAILURE_BODY_LIMIT`. Never put body text in an error except as a `Reason`.
- A 409 is not definite and is never retried.
- `reconcile` sends GET requests only and adopts a group only with the exact
  name and `horizon-operation` tag.
- Do not add credential sources, power actions, run commands or UI, CLI or MCP
  callers as part of this client.
