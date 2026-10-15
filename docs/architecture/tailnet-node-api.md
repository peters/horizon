# Desktop recorded-node HTTP prerequisite

The desktop SDK supplies `NodeApi` for a trusted desktop controller.
It can delete an exact captured device ID after provider compute and storage absence.
The SDK supplies no UI, CLI or MCP operation itself.
The owning controller must authenticate the owner and examine the locked receipt.
The SDK grants no owner or workspace authority.

Inputs are the saved local tailnet ID, recorded device ID and remaining HTTP budget.
The SDK bounds each device ID and encodes it as one path segment.
It does not infer IDs from names, tags or inventory.
A trusted library caller can deliberately supply another ID.
The owning controller must prevent that operation.

API credentials stay in the desktop OS store.
API status and node requests use the same private resolver under the existing tailnet lock.
Active metadata selects its exact managed slot.
Disabled metadata prevents staged fallback.
Only absent metadata permits the staged account for the exact saved ID.
Missing managed slots, catalog removal, malformed bindings and failed recovery never select another credential.

Enrollment credentials remain separate from API credentials and recovery.
External OS-store or file changes are outside the cooperating Store-writer guarantee.
The SDK adds no public MCP mutation or secret getter.
The worker receives no API credential, OAuth secret or bearer token.

The production origin is `https://api.tailscale.com/api/v2`.
The client refuses redirects and ambient proxies.
Headers and response bodies have a 16 KiB limit.
Each HTTP request has a 15-second limit within the shared budget, capped at 30 seconds.
The shared deadline starts before the Store lock and credential read.
Filesystem and OS-store calls keep their platform behavior; those calls have no hard wall-clock limit.

Only tests can substitute an offline HTTP origin.
Personal tokens and OAuth clients are supported.
OAuth exchange requests `devices:core` and `tag:horizon-worker`.
See the official [OAuth client documentation](https://tailscale.com/docs/features/oauth-clients).
Secret material remains private and uses zeroizing storage at owned boundaries.
Typed results exclude response bodies and raw transport or credential-store errors.

HTTP 200 or 204 returns `ConfirmedDeleted` to the trusted caller.
That caller must save confirmation in its owning record.
Missing credentials, access denial, invalid OAuth replies, transport failures and unexpected responses return a fixed `Retained` reason.
Every 404 remains unconfirmed, including a credential from another tailnet.

OAuth HTTP 401 and 403 report denied access.
OAuth HTTP 408, 429 and 503 report transport unavailability, timeout or rate limiting.
Other non-200 OAuth replies report an unexpected response, with no subsequent DELETE request.
An expired shared budget prevents DELETE dispatch even after a valid OAuth reply.

The SDK changes no owning ledger and has no name-based fallback.
Core integration is described in [recorded-node cleanup](tailnet-node-cleanup.md).
That integration needs first-head and final-head real-input checks.
