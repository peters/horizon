# Worker image destinations

This document defines the image destination boundary for epic #1597.
The destination policy is a reusable module in `horizon-cloud`.
It does not authenticate, send network requests or publish an image.
The launch pipeline does not use automatic destinations yet.

## Destination identity

A destination contains an explicit registry repository and a non-secret publisher identity.
An automatic GHCR destination uses the host's image-publishing account.
The source repository cannot select that account.
Personal destinations retain a lowercase account name and that account's own namespace.
An observed login with different letter case still names the same account.
A machine-local registry destination names the exact credential reference that permits publication.
Organization destinations use a machine-local registry binding.

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
