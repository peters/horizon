# Tailnet API credential storage boundary

## Trusted authority

The Horizon host and its OS credential store are trusted.
The host store supports a personal API token or OAuth client for each saved tailnet.
The saved local tailnet ID names a binding, not proof of remote credential authority.
Profiles that reuse that ID intentionally share its API binding; a profile name
does not create credential isolation.
API credentials stay on this computer. Enrollment transports receive no API secret.

API credentials use the OS-store service `horizon-cloud-tailnet-api`.
Enrollment auth keys use the separate service `horizon-cloud-tailnets`.
API save, replacement and removal leave the enrollment key intact.
Auth-key replacement leaves the API binding intact.
Removing a saved tailnet cleans both bindings under the shared catalog lock.

## Durable binding changes

The API binding file and recovery journal contain nonsecret metadata only.
Each save writes a fresh opaque slot before it commits the binding.
Recovery discards the new slot before commit, or retires the old slot after commit.
Cleanup acts only on the exact recorded accounts of the saved local tailnet.
A failure cannot authorize cleanup of another tailnet's slot.
An explicit removal suppresses staged fallback, also after uncertain cleanup.

## Staged credentials

A saved tailnet without a managed API binding can use its staged OS-store entry.
That entry uses the API service and the exact saved local tailnet ID.
Personal-token JSON has `kind: personal_token` and `token`. OAuth JSON has
`kind: oauth`, `client_id` and `client_secret`. The personal token must start with
`tskey-api-`; an enrollment `tskey-auth-` key is not an API credential.
The resolver accepts bounded fields and refuses contradictory or unknown data.
It never searches another tailnet, namespace or enrollment key as a fallback.
A missing managed secret does not fall back to a staged older token.
Status reads do not migrate or rewrite a staged credential.
They can finish a prior recorded binding recovery, including retirement of the
exact obsolete OS-store slots. Missing and disabled metadata are different: an
explicitly disabled entry reports missing without trying staged fallback.

## Untrusted input and public output

Credential drafts and stored bytes require bounded validation.
Secret values use zeroizing storage and have no public read or debug API.
Public status returns only missing or stored credential kind.
Locked, unreadable or malformed storage returns a value-free error.
It is not proof that a credential is missing.
No secret enters project configuration, persisted application state, worker payloads or logs.
Callers must run filesystem and OS-store operations off the UI thread.
MCP must not accept raw API secrets or expose saved values.

## Scope of the foundation

This foundation supplies host storage APIs for credential adapters.
It adds no API editor or new public CLI/MCP credential operation. The separate
status/editor milestones use these APIs; no interface may import/read a raw API
credential through MCP. Worker enrollment remains auth-key based and requires a
separately qualified marker-4 image. API storage does not update a worker pin.
Stored presence means the resolver can read and parse the OS-store entry.
It does not test validity, expiry, selected scopes, tags or API reachability.
Saving does not contact Tailscale, mint a token or change remote access.
This milestone does not record enrollment IDs or remove tailnet nodes.

For an OAuth client, select `devices:core` and `tag:horizon-worker` at Tailscale.
Do not assume that tag selection restricts which devices the credential can delete.
Treat its device-removal authority as able to reach other tailnet devices.
Personal API tokens can have broad API authority and an expiry date.
Future cleanup needs a separate recorded-device-ID boundary and qualification.
See [Tailscale trust credentials](https://tailscale.com/docs/reference/trust-credentials)
and [OAuth clients](https://tailscale.com/docs/features/oauth-clients).
