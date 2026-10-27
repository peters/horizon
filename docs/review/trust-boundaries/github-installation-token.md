---
applyTo: "crates/horizon-cloud/src/github.rs,crates/horizon-cloud/src/github/installation.rs,crates/horizon-cloud/src/github/installation/**,crates/horizon-core/src/cloud_runtime/github.rs,crates/horizon-core/src/cloud_runtime/github/connect.rs"
---

# GitHub App installation token boundary

Read the module comment in
`crates/horizon-cloud/src/github/installation.rs` for the guarantee and the limits.
This computer, its cloud settings file and the app's private key file are trusted.
GitHub is not: every byte of a GitHub answer is untrusted input.

## What is trusted

- The app ID and the slug in the cloud settings.
- The private key file that `github.private_key_file` names. It is a private
  regular file with mode 0600 or stricter, readable only by this user. Horizon
  writes it when **Connect GitHub** creates the app, from the `pem` that GitHub
  answers the manifest conversion with, at `credentials/github-app-<app>.pem` in
  the cloud root. A key file that is already there is kept, never replaced, and a
  person can name a key file that they made themselves instead.

## What the code guarantees

- The manifest answer's `pem` reaches one private file in `credentials/` under the
  cloud root, in a directory with mode 0700. The file is a new entry with mode
  0600, and the write refuses an entry that is already there instead of following
  or truncating it, so an existing key file is kept. The app's client secret is
  written the same way, to the file that `github.client_secret_file` names.
- A connect that fails after it wrote one of the two files removes what it wrote,
  a partly written file included, and reports the failure that refused the
  connect. A key file that was already there is kept unchanged through that
  failure, because the connect never wrote it.
- The private key stays on this computer. Only a signature over the app JWT
  leaves it: the key goes into no settings file, cloud record, worker payload,
  SSH command, environment or evidence file. Cloud settings name the key file by
  path only. The `pem` of a manifest answer reaches that one file and nothing
  else.
- The JWT names the app in `iss`, was issued 60 seconds before now and expires
  540 seconds after now, which is inside GitHub's limits of 10 minutes and one
  minute in the past.
- A token request names the repositories and exactly three permissions:
  `contents: write`, `pull_requests: write` and `metadata: read`. The request
  never omits the repository list, so the token never reaches every repository
  of the installation.
- An answer that grants a permission that the request did not name is a
  malformed answer, and no token is kept.
- The installation is read one page at a time, at most 20 pages of 100. A full
  page at the cap is an error, never a partial list.
- The expiry is read from the answer, never counted here.
- The token has no refresh token, so a holder of the token alone cannot renew
  it. Renewal needs the key, and the key stays here.

## Report these failures

- The private key, a JWT, an installation token or the app's client secret
  reaches `Debug` output, an error message, a log line, a progress line, a
  command argument, an environment of a host process, or a test fixture other
  than a synthetic value.
- A token request names another permission, or leaves out the repository list.
- A token answer with a wider grant than the request becomes a token.
- The installations read has no page cap, or reports a partial list as
  complete.
- A refused answer becomes a token, or an error text repeats a value that
  GitHub sent.
- The private key file is read without the check that it is a private regular
  file, or a relative path is accepted.
- The manifest answer's `pem` is written to a file that another user can read or
  replace, or an existing key file is replaced rather than kept.

## What this does not guarantee

- Horizon does not shorten the lifetime below the one that GitHub answers, and
  it does not renew or revoke an installation token. The token works until
  GitHub ends it, or until the app's owner deletes the app.
- Horizon trusts the answer that GitHub gives when the installation does not
  cover a repository, or when the app does not hold a permission that the
  request names. It does not read the repository list of the installation
  itself.
- Horizon does not read the numeric user ID of the bot. GitHub attributes a
  commit to the bot account only when the author address holds that ID. A commit
  with the name `<slug>[bot]` and its address names the bot, and GitHub does not
  attribute that commit to it.
- The parsed RSA key lives in `ring`'s own storage, which does not wipe it on
  drop. The PEM text and the DER bytes of the key are wiped.
- This boundary covers one installation of the app. It does not cover what a
  cloud does with the token: a later change that gives a token to a worker or a
  cloud record needs its own boundary text.
