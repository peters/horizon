---
applyTo: "crates/horizon-cloud/src/hetzner/firewalls.rs,crates/horizon-cloud/src/hetzner/tests/firewalls.rs,scripts/check-hetzner-api.py"
---

Review this default-off provider component against
`docs/architecture/exposure-firewalls.md`. Expected scope is comparison data.
Existing mutations must freshly verify the exact ID, full ownership labels and
no attachment or only the original numeric server. Never accept selectors or
foreign targets. Creation is unattached, one POST only; uncertain responses require
reconciliation, not retries. IDs precede action waits. Successful actions are not
proof of current state or physical gate closure. Do not assume the SDK implements
owner approval or atomic preflight/mutation. The management subtree has its own
boundary and review instructions; lower-level operations alone preserve no policy.
