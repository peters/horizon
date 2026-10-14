---
applyTo: "crates/horizon-cloud/src/tailnet.rs,crates/horizon-cloud/src/tailnet/**,crates/horizon-core/src/cloud_runtime/tailnet.rs,crates/horizon-core/src/cloud_runtime/tailnet/**,crates/horizon-core/examples/cloud_deploy.rs,crates/horizon-core/examples/cloud_deploy/tailnets.rs,crates/horizon-ui/src/app/tailnets.rs,crates/horizon-ui/src/app/tailnets/**,crates/horizon-ui/src/app/cloud_panel/production/companions/agent.rs,crates/horizon-ui/src/app/cloud_panel/production/companions/agent/**,crates/horizon-browser-mcp/src/server.rs"
---

# Tailnet API credential status boundary

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
- Saved-tailnet deletion leaves permanent API tombstones, or recovery prunes a
  disabled record before catalog deletion commits. Metadata cleanup failure must
  retain the deletion journal; active/pending or sibling bindings must survive.

Also report concrete inputs and wrong results when:

- Status rendering performs filesystem, keychain or network work.
- Pending reads or failures disappear when Settings closes.
- A stale reply restores a removed network's status.
- Settings completion leaves MCP on an older observation until Settings closes.
- A fresh catalog/status read starts before a closed API operation barrier completes.
- MCP status requires Settings open, reports pending/unavailable as missing,
  or uses another root's cached catalog.
- CLI/MCP exposes a value, imports a secret or submits a new mutation/remote cleanup.
  Completing an exact prior recorded local binding recovery is allowed.

These limits are deliberate:

- Status adapters are read-only; no API credential editor or import operation is present.
- Save does not call Tailscale or validate remote scopes, expiry or tags.
- This milestone does not record enrollment IDs or remove tailnet devices.
- Tag selection is not proof of provider-side restriction of device deletion.
- The OS store remains the secret authority; JSON is nonsecret binding metadata.

Do not request the deferred credential editor or remote node cleanup as fixes to status parity.
