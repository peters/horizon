---
applyTo: "crates/horizon-core/src/cloud_runtime/github/packages.rs,crates/horizon-core/src/cloud_runtime/github/signin.rs,crates/horizon-core/src/cloud_runtime/deployment/git_credentials.rs,crates/horizon-cloud/src/github/api.rs,crates/horizon-ui/src/app/cloud_panel/production/cards/github.rs"
---

# The cloud's package read access (host side)

Read "Package read access" in `docs/cloud-workspaces.md` for the guarantee and limits,
and "Trust boundary of the package token" in `docs/upkeep.md` for the worker side. The
person's own GitHub authorization and GitHub's answers about the token are trusted;
everything the worker reports is only a hint for whether to ask.

Report these failures:

- The chain reaches anything but root on the worker over SSH stdin: argv, a log, the
  deployment output, the cloud board, `settings.json`, a file on this computer, or a test
  fixture other than a synthetic value. Horizon keeps no copy after the install.
- The device sign-in asks for any scope but `read:packages`, or a chain whose scopes are
  not exactly `read:packages` (from the `X-OAuth-Scopes` header) reaches the worker.
- The host's image publishing chain (`horizon-github-packages.json`, `write:packages`)
  is read, copied or sent to a worker by this code.
- A client secret enters the payload or the code: the worker renews with the client ID.
- A skipped, refused or failed package sign-in fails the deployment, or passes without
  an output line that says package checks need a person.
- An organization that limits OAuth apps is not named, with its grant page, in the output.
