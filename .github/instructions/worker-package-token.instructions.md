---
applyTo: "examples/cloud-worker/horizon-worker-github-packages,examples/cloud-worker/horizon-worker-github-api,examples/cloud-worker/horizon-worker-github-api-rest,examples/cloud-worker/test_github_packages.py,docs/testing/procedures/worker-package-token.md"
---

# Trust boundary of the worker's package token

GitHub's package API and its NuGet registry accept only a token with the
`read:packages` scope, such as an OAuth app's token. Horizon gives such a token to root on the worker
(`horizon-worker-github packages`, JSON on stdin). The user-facing description is in
`docs/upkeep.md`, section "Trust boundary of the package token", and in
`examples/cloud-worker/README.md`, "Package token".

## Trusted: the token that Horizon gives root, and the cloud's grants

- The token arrives only on root's stdin. It never enters argv, a log, the request log,
  a refusal or the `status` output (which says only whether a token is held).
- Root keeps it only in root-private copies (`common.write_private`) and reads them with
  `common.read_json`. An install or removal holds `packages.lock` for all copies, and each
  read shares it, so overlapping installs never leave copies that differ. A copy that is not private, cannot be parsed, or differs from the
  other copy serves no token. A storage that does not take a new token loses its older
  copy.

## Untrusted: every request of an agent

Requests from uid 10001 reach the API broker with any method, path, query and owner.
Check that:

- Only `GET` and `HEAD` of `/orgs|users/OWNER/packages` (with exactly one
  `package_type` of `nuget` or `npm`), `.../packages/TYPE/NAME`, `.../versions` and
  `.../versions/ID` get the package token, with TYPE exactly `nuget` or `npm`. A
  container image, any other type, a write, a restore and a delete are refused before
  any request reaches GitHub. `rest.split` already refuses dot segments, encoded
  slashes, backslashes and NUL.
- OWNER must be the owner (compared with `casefold`) of a repository in the
  `grants` or `cloud_grants` of a chain that may serve (`store.load(serving=True)`,
  state `ok`), or of the static binding. A revoked chain and a task grant never open
  an owner's packages. `clear` removes the token before the chain.
- A package route never gets the chain's or the static binding's token, and a
  repository route never gets the package token.
- `Exchange.fresh` checks the token again right before it goes out, so a token that
  Horizon removed or replaced, or an owner that lost its grant, stops the request.
- Agents never get the token: no environment variable, no file in their home and no
  `NuGet.Config` with a password.
