---
applyTo: "crates/horizon-cloud/src/tailnet.rs,crates/horizon-cloud/src/tailnet/**,crates/horizon-core/src/cloud_runtime/tailnet.rs,crates/horizon-core/src/cloud_runtime/tailnet/**"
---

# Tailnet API credential foundation boundary

Read `docs/architecture/tailnet-api-credentials.md` for the guarantee and limits.
The host and OS store are trusted. Drafts and stored bytes require bounded checks.
API credentials stay on the desktop and are independent of enrollment auth keys.

Report these failures:

- API save, replacement or removal changes an enrollment auth-key binding.
- Auth-key replacement changes an API binding.
- Cleanup touches a slot of another tailnet, or uncertain removal revives staged fallback.
- Recovery loses the previous credential before the new binding commits.
- A missing managed slot falls back to an older staged credential.
- A secret reaches debug output, public metadata, a worker payload, project
  configuration, command arguments or logs.
- Validation accepts extra, contradictory, null or unbounded credential fields.
- A locked or malformed active/staged OS-store entry produces successful missing status.
  Explicit disabled metadata is known missing without a staged read.
- A status read migrates the staged binding or starts a new mutation. Completing
  an exact already recorded recovery remains allowed.
- The docs claim stored presence proves scope, expiry or API validity.

These limits are deliberate:

- The foundation supplies shared host storage APIs, not a new credential editor.
- Save does not call Tailscale or validate remote scopes, expiry or tags.
- This milestone does not record enrollment IDs or remove tailnet devices.
- Tag selection is not proof of provider-side restriction of device deletion.
- The OS store remains the secret authority; JSON is nonsecret binding metadata.

Do not request deferred UI or runtime cleanup as fixes to the storage foundation.
