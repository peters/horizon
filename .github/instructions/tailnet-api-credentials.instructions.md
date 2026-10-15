---
applyTo: "crates/horizon-cloud/src/tailnet.rs,crates/horizon-cloud/src/tailnet/**,crates/horizon-core/src/cloud_runtime/tailnet.rs,crates/horizon-core/src/cloud_runtime/tailnet/**,crates/horizon-core/examples/cloud_deploy.rs,crates/horizon-core/examples/cloud_deploy/tailnets.rs,crates/horizon-ui/src/app/tailnets.rs,crates/horizon-ui/src/app/tailnets/**,crates/horizon-ui/src/app/cloud_panel/production/companions/agent.rs,crates/horizon-ui/src/app/cloud_panel/production/companions/agent/**,crates/horizon-browser-mcp/src/server.rs,crates/horizon-browser-mcp/src/controller/tailnet_credential.rs,crates/horizon-browser-control/src/manifest/tailnet_credential.rs,crates/horizon-browser-control/src/manifest/provider_usage.rs,crates/horizon-ui/src/app/browser_provider_usage.rs,crates/horizon-ui/src/app/settings/mod.rs,crates/horizon-ui/src/app/cloud_panel.rs,crates/horizon-cloud-worker/src/remote.rs"
---

# Tailnet API credential setup and status boundary

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
- Reopening starts an independent reader while a retained write or barrier is pending,
  or hides its completed write failure from the reopened UI.
- The passive production mirror drops its known catalog or cached cloud selection.
- Connections starts a selection save on the passive production mirror while
  Settings owns IO, leaving its result unprocessed or its error hidden.
- A root change refreshes the wrong store, allows new editor actions, or adopts
  another root's pending operation or observation.
- Either cloud-enabled credential replies or feature-disabled refusals write
  completion files on the UI thread instead of the shared `HostIo` worker.
- A stale reply restores a removed network's status.
- Settings completion leaves MCP on an older observation until Settings closes.
- A fresh catalog/status read starts before a closed API operation barrier completes.
- MCP status requires Settings open, reports pending/unavailable as missing,
  or uses another root's cached catalog.
- CLI/MCP exposes a value, imports a secret or directly authorizes a binding mutation/remote cleanup.
  Completing an exact prior recorded local binding recovery is allowed.

These limits are deliberate:

- Status adapters remain nonsecret. Management requests require an explicit native owner choice.
- Save does not call Tailscale or validate remote scopes, expiry or tags.
- API credential management does not capture enrollment IDs or remove tailnet devices.
- Tag selection is not proof of provider-side restriction of device deletion.
- The OS store remains the secret authority; JSON is nonsecret binding metadata.

Also report:

- A workspace actor, same UID, tool argument or pending request itself authorizes a global binding write.
- An unknown root/ID, changed name, different operation or expired/cancelled request reaches the write worker.
- One actor reads, cancels or retargets another actor's request.
- A removal starts without an exact native owner decision, or a request opens Settings.
- A running write is reported cancelled, or matching kind is treated as proof of completion.
- Settings close loses the operation result or lets stale status replace its completion.
- A write result received after the original retention deadline disappears before its caller can read it.
- Catalog deletion makes an original caller's retained request result unreadable, or result recovery authorizes a new write.
- The public schema accepts Set/replace, raw secret values or a secret getter.
- Standalone workers or feature-disabled hosts accept desktop removal requests.

Worker receipt capture has its own boundary. API credential management grants no
remote node cleanup authority.

Flag a successful status read that erases an unrelated API write failure, or a
write result that clears a catalog/save or reader failure. Closed jobs must retain
their read/write kind. Do not copy reader errors into generic parent settings
errors. The UI shows each error once; receipt state still follows Store completion.
