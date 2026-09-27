# Private registry bindings

Open **Cloud > Cloud settings > Private container images**. Add the exact image
repository, for example `ghcr.io/example-team/worker`, and separate publishing and
worker-pull logins. Publishing is optional for existing images. Confirm that the
pull grant is read-only and limited to the intended repository. Expiry is optional
RFC3339 (for example `2027-01-01T00:00:00Z`); blank means **unknown**, not perpetual.

Saving writes private, immutable credential files under the machine's cloud account
directory. Settings contain file references and generation identities, never tokens.
Saving does not publish, allocate compute or transfer credentials. Reopen settings,
enter an existing immutable `repository@sha256:...` image and select **Validate pull
access**. This checks pull access and then prepares the named provider pull binding.
Only that pull credential is sent to the provider. Publishing credentials remain
local. Validation and deployment share the same policy.

For GHCR, use a dedicated classic token with only `read:packages`, authorized for
the organization's package and SSO policy where required. Broader or unobservable
scopes are refused. Horizon verifies scopes through the issuer and reads the exact
manifest with the pull login. Classic-token scope is not a repository-level ACL:
the account/package permissions must also restrict access appropriately. See
[GitHub's container registry documentation](https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry).
Other registries retain support; their read-only scope is explicitly owner-confirmed
because a generic Docker registry does not expose the issuer's full grant. A pull
probe proves image readability, not absence of write permission. Horizon neither
broadens grants nor falls back to a public image after an explicit binding fails.

The deployment pipeline uses the selected publishing login for build/push, then
checks the immutable result using the separate pull login before allocation.
Image-only deployments do not require a live publishing credential. Exact repository
matching prevents neighboring repositories on the same registry from inheriting a
grant. Unconfigured registries keep their existing Docker/provider settings.
Use explicit `docker.io/...` references when configuring Docker Hub bindings.

## Rotation, revocation and recovery

Enter a replacement pull credential and save. This creates a new generation and
retains previous handles. Validate the replacement before choosing **Revoke previous**.
Existing worker specifications keep their original identity; fresh allocations use
the validated current generation. Do not retire access still needed by an existing
worker's future restart. Horizon does not mutate running workers during rotation.

**Revoke pull binding** removes provider access for that exact generation. It does
not revoke the token at its issuer or remove credentials a running process already
holds. Revoke the issuer token separately when appropriate. A revoked current
generation stays blocked even if an older settings snapshot is used; rotate to
establish new access.

Uncertain creation never repeats its POST. **Reconcile** can recover a uniquely
named binding; an empty listing leaves uncertain creation fenced. Uncertain deletion
retains its revocation intent until absence is observed. Do not delete journals to
clear uncertainty. Journals are tied to the original compute credential; reconcile
using that credential before changing the compute account/key. Current implementation
requires a Unix host for durable cloud journals, matching cloud deployment support.
Provider binding operations follow the documented [create](https://docs.runpod.io/api-reference-v2/registries/create-a-container-registry-credential)
and [delete](https://docs.runpod.io/api-reference-v2/registries/delete-a-container-registry-credential) APIs.

## CLI and MCP

Build the existing helper with `cargo build -p horizon-core --example cloud_deploy`.
Import a private JSON description with credential **file paths**, never literal tokens:

```json
{
  "repository": "ghcr.io/example-team/worker",
  "read_only_confirmed": true,
  "publish": {"username": "publisher", "secret_file": "/private/push-token", "expires_at": null},
  "pull": {"username": "reader", "secret_file": "/private/pull-token", "expires_at": null}
}
```

```sh
target/debug/examples/cloud_deploy registry-bind /private/cloud/settings.json /private/binding.json
target/debug/examples/cloud_deploy registry /private/cloud/settings.json /private/action.json
```

Import uses the same private settings transaction and rotation rules as the form.
Action JSON is one of:

```json
{"operation":"verify","image":"ghcr.io/example-team/worker@sha256:<64-hex-digits>"}
{"operation":"status","repository":"ghcr.io/example-team/worker","generation":"<saved-generation>"}
{"operation":"reconcile","repository":"ghcr.io/example-team/worker","generation":"<saved-generation>"}
{"operation":"revoke","repository":"ghcr.io/example-team/worker","generation":"<saved-generation>"}
```

For MCP, configure the built helper as a stdio server with arguments
`registry-mcp /private/cloud/settings.json`. Its `cloud_registry` tool accepts the
same action objects. The settings path is fixed at startup; tool calls cannot supply
secrets, settings paths or provider endpoints. Configure/import bindings through the
form or CLI first. No UI needs to be open. Cancellation preserves pending mutation
journals; use status/reconcile to inspect an interrupted operation.

Status includes the generation, provider state, last validated image, scope proof,
configured pull expiry even before verification, observed expiry and verification timestamp. It is historical evidence,
not a guarantee that the issuer has not revoked the token since. Deployment validates
again. Docker with buildx and network access to the registry are required for image
validation. The helper uses the workspace's existing MCP/runtime dependencies only
for its development executable; no new runtime dependency is added to horizon-core.

Deployment retries retain the selected private registry generation. Removing its
binding cannot switch a prepared private image to ambient authentication: restore
or rotate the binding before retrying. Existing worker reconciliation does not
require the removed local credential.
