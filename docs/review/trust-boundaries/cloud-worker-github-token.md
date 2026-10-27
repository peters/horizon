---
applyTo: "crates/horizon-core/src/cloud_runtime/github/installation.rs,crates/horizon-core/src/cloud_runtime/github/worker.rs,crates/horizon-core/src/cloud_runtime/deployment/git_credentials.rs"
---

# Cloud worker GitHub token boundary

Read the module comment in
`crates/horizon-core/src/cloud_runtime/github/installation.rs` and the token
boundary [`github-installation-token`](github-installation-token.md), which covers
the mint itself.

The create of an agent's cloud has no person to sign in to GitHub. When cloud
settings hold the app's private key, Horizon gives that cloud's worker an
installation token instead.

## What is trusted

- The cloud settings file of this computer: the app, and the repositories of the
  agent lifecycle policy.
- The app's private key file, and the record of a cloud that an agent created.
- The worker's root service. Its answer is parsed, never shown as deployment
  output. Every byte of a GitHub answer is untrusted input.

## What a worker holds

- One installation token of the app, for exactly the repositories of the agent
  lifecycle policy on one account. It is valid for at most one hour.
- The token arrives as payload version 2 on SSH stdin, in memory. It never enters
  a command argument, an output, a log line or a named file.
- The worker's root service keeps the chain in a file with mode 0600 in a
  directory with mode 0700. Git reaches GitHub through a loopback proxy that adds
  the token, and `gh` reaches it through the API broker, which holds the token.
  An unprivileged agent user does not hold the token itself.
- The worker cannot renew the token. The chain of version 2 has no refresh token,
  the service answers every refresh check with `host_only`, and the app's private
  key is not on the worker. Only this computer mints the next token.

## What the code guarantees

- The payload of version 2 is `version`, `client_id`, `login`, `author_name`,
  `author_email`, `grants` and `chain`. The chain is exactly `access_token`,
  `access_expires_at` and `kind: "installation"`. It holds no refresh field and
  no client secret.
- `login`, `author_name` and `author_email` are the bot of the app, never a
  person. The payload of version 1 keeps its shape.
- The token request names the policy's repositories and no other repository, and
  the mint refuses a policy of more than one account.
- A policy path without a GitHub remote, an app that is not installed, and a
  repository that the app does not reach refuse the create before a card exists
  or a worker is rented. The refusal is a typed error that names the repository.
- The mint reads the app's private key and calls GitHub on a host thread, never
  on the frame thread. The create mints once to refuse, and drops that token. The
  deployment mints the token that the worker holds, so the token lasts an hour
  from the install and not from the answer, which an image build may spend.
- The worker's status that names an ended installation token
  (`installation_token_expired`) is its own status, not a missing or a stopped
  chain. Horizon reports that only this computer installs a new one, and a status
  field or state that Horizon does not know stays ignored, as before.
- No value of the token or the key reaches output. The log lines of this path name the
  bot login, a repository and a count.

## Report these failures

- A token, a JWT, the app's private key or the client secret reaches `Debug`
  output, an error message, a log line, a progress line, a command argument, an
  environment of a host process, or a test fixture other than a synthetic value.
- Payload version 2 carries a refresh field or a client secret, or the chain kind
  is missing.
- Payload version 1 changes shape, or a worker receives a version it refuses.
- An ended installation token is reported as an absent or an unavailable chain.
- A mint or an install runs on the frame thread.
- A token request names a repository that the policy does not list.

## What this does not guarantee

- The policy's repository list is the only scope. The token reaches every
  repository of the policy, not only the repositories of one cloud's checkouts.
  A sibling checkout outside the policy keeps no access.
- Horizon does not revoke the token when a cloud stops or is deleted. The token
  works until GitHub ends it, at most an hour after the mint, or until the app's
  owner deletes the app.
- A worker's root user can read the token file, as root reads every file of that
  worker. Horizon does not protect the token from root, from the provider's
  console or from a fault in the worker image.
- An agent on the worker uses the access of every repository of the policy until
  the token ends. Horizon cannot narrow it below the policy.
- Horizon does not read the repository list of the app's installation. A refusal
  for a repository that the app does not reach names the repositories of the
  policy, and not the one repository that GitHub refused.
- The create mints before a worker exists, so a mint that fails after the
  deployment started fails the deployment. It is not a create refusal.
- This boundary covers the token of one agent's cloud. It does not cover the
  person's sign-in, which `github-installation-token` and the card's Connect
  GitHub path keep.
