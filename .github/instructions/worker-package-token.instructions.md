---
applyTo: "examples/cloud-worker/horizon-worker-github-packages,examples/cloud-worker/horizon-worker-github,examples/cloud-worker/horizon-worker-github-api,examples/cloud-worker/horizon-worker-github-api-rest,examples/cloud-worker/test_github_packages.py,docs/testing/procedures/worker-package-token.md"
---

# Trust boundary of the worker's package token

GitHub's package API and its NuGet registry accept only a token with the
`read:packages` scope, such as an OAuth app's token. Horizon gives such a token to root on the worker
(`horizon-worker-github packages`, JSON on stdin). The user-facing description is in
`docs/upkeep.md`, section "Trust boundary of the package token", and in
`examples/cloud-worker/README.md`, "Package token".

## Trusted: the chain and the repositories that Horizon gives root

- The chain arrives only on root's stdin with the cloud's own `repositories` (1 to 50).
  No token or refresh token enters argv, an environment, a log, the request log, a
  refusal, a reply or the `status` output (state, times and repositories only).
- `install` asks GitHub for the token's scopes (`scopes_of`, `X-OAuth-Scopes`) and keeps
  the chain only when they are exactly `read:packages`. A renewal (`refresh_once` with
  `scope`) whose reply names any other scope clears the store. No client secret exists
  anywhere in this path.
- Root keeps it in its own `common.Store` (`packages.store_for`), with the GitHub chain's
  rules for private copies, serials, pending writes and clear marks. One store object per
  GitHub store, so the broker sees a rotated chain that only the renewal's memory holds.
- `install` replaces the chain under the store lock; a replacement that fails for any
  reason (request, scope, GitHub unreachable, storage) clears the
  store, so no older chain is served. `packages.clear` writes a clear mark above every copy
  when a copy cannot be removed. `horizon-worker-github clear` removes it inside the
  GitHub store lock.

## Untrusted: every request of an agent, and GitHub's package replies

Requests from uid 10001 reach the API broker with any method, path, query, body and
owner. Check that:

- Only `GET` (not `HEAD`) without a body, with only the fields of `packages.REQUEST_FIELDS`
  (no range, condition or encoding), of `/orgs|users/OWNER/packages` (with exactly one
  `package_type` of `nuget` or `npm`), `.../packages/TYPE/NAME`, `.../versions` and
  `.../versions/ID` get the package token, with TYPE exactly `nuget` or `npm` and only the
  query keys of `PACKAGE_QUERY`. A container image, any other type or key, a write, a
  restore and a delete are refused before any request reaches GitHub. `rest.split`
  already refuses dot segments, encoded slashes, backslashes and NUL.
- OWNER must be the owner (compared with `casefold`) of one of the chain's own
  `repositories`. No grant opens an owner: not `grants`, not `cloud_grants` that an agent
  asked for, not a task grant, not the static binding.
- The broker never follows a redirect; a 3xx goes back to gh without the token, with an
  empty body and only `Location`. A 206 or a `Content-Range` reply is refused.
- A package reply is read uncompressed (at most `MAX_REPLY`), parsed as JSON, and its
  `repository` objects are kept only for the chain's own repositories, and every string
  that names another repository by its GitHub address becomes null (`filtered`). A
  reply that is not JSON, larger or holding the token is refused, never relayed.
- A package route never gets the chain's or the static binding's token, and a
  repository route never gets the package token.
- `Exchange.fresh` checks the token again right before it goes out.
