# Worker image destinations

This document defines the image destination boundary for epic #1597.
The destination policy is a reusable module in `horizon-cloud`.
It does not authenticate, send network requests or publish an image.
The launch pipeline does not use automatic destinations yet.

## Host publication preparation

The host has an operation preparation helper in `github::publish::publication`.
It authenticates the publishing account and returns non-secret account metadata with a private Docker configuration.
The account metadata contains the provider's immutable user ID and its login name.
The helper rejects another user ID, including a reused login. Letter case does not change the account.
An account rename is refused because it could change the selected namespace.

The operation owns its temporary Docker configuration until it finishes.
It writes only its authenticated publishing login there, not into the host's shared Docker configuration.
A later sign-in or another publication cannot replace that operation's credentials.
The temporary configuration is removed when the operation is dropped.
On Unix, the directory is created with mode 0700 and its login file uses mode 0600.
Cancellation is checked before local work, after the token lookup and before the result is returned.
Credentials are neither printable nor serializable through this helper.
Expected account metadata is untrusted and must pass validation before the login is prepared.
The helper can renew the same account's operation login before a push. A long build must
call this renewal checkpoint; preparation alone does not guarantee token validity through
a later push. The helper refuses another current account without replacing its private login.
It does not supply an account-switch UI or per-account storage. The launch integration must
provide an explicit sign-in path for a recorded account before activation.
This helper has no launch caller yet. The launch integration follows separately.

## Destination identity

A destination contains an explicit registry repository and a non-secret publisher identity.
An automatic GHCR destination uses the host's image-publishing account.
The source repository cannot select that account.
Personal destinations retain a lowercase account name and that account's own namespace.
An observed login with different letter case still names the same account.
A machine-local registry destination names the exact credential reference that permits publication.
Organization destinations use a machine-local registry binding.
Enterprise Managed Users require an organization destination because GitHub does not provide personal package storage for those accounts.
See [the GitHub registry documentation](https://docs.github.com/en/enterprise-cloud@latest/packages/working-with-a-github-packages-registry/working-with-the-container-registry).

The package name contains the repository name, profile name and a short hash of the source identity.
The hash separates equal names from different source hosts and owners.
Normalization cannot turn repository text into a registry address or an account selection.
The source identity must not contain credentials. Host adapters remove credentials before they call the policy.

## Trust boundary

Repository names, profile names and persisted records are untrusted input.
The policy rejects malformed accounts, credential references and registry repositories.
Destinations omit tags, digests and credential material.
The policy describes a selection. It does not prove that the account has permission to publish.
The host must check permission and retain the selected account before a build.

Hosts must check the recorded publisher before each publication attempt.
A reconnect, retry or account change must not select another destination.
Repository configuration cannot name credential references or change an existing cloud's publisher.
Workers receive only their authorized pull access. Publication credentials stay on the host.

## Registry independence

A bound destination can use a different registry from the source Git service.
The contract accepts nested project paths and explicit registry ports.
GitLab account connection and project discovery are future work.
The current task does not add those operations.

## Bound CPU worker pulls

An exact Container registry binding takes precedence over the legacy CPU-provider login.
Before allocation, the read-only grant must verify the worker's immutable image digest.
The provider adapter retains that image identity and the verified material in memory.
A local credential-file change cannot substitute another login in host user data.
Missing verification or a different immutable digest stops allocation before provider contact.
Unbound images retain the existing legacy-provider behavior.
Temporary registry Docker directories are private at creation on Unix.
These checks prove the immutable image read at that moment; they cannot guarantee future issuer availability.

This prerequisite retains the existing Container registry journal requirement for a configured RunPod account.
A host-only journal is a required follow-up before portable publishing activates on a Hetzner-only machine.
