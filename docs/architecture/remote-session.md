# Remote session transport

Issue #1368 adds an independent Rust transport package at `crates/remote-session`.
The first outcome supplies SSH command execution with an explicit host key pin.
Horizon does not use this package for its existing connections yet.
PTY attachment, Mosh, history, forwarding, SFTP and Git integration remain separate outcomes.

## Package boundary

The package has its own version and MIT OR Apache-2.0 license.
It has no dependency on Horizon, a provider, a UI library or a global runtime.
The caller supplies a Tokio runtime, the destination, credentials and the expected host key.
The package does not read the user's SSH configuration or credential store.
It does not start an external SSH client or change a known-hosts file.

One command owns one SSH connection in this first outcome.
The caller sends input through a bounded queue and receives separate channel events.
Output, extended output, EOF and exit status retain their SSH meanings.
The caller can cancel the command and wait for its owned transport to stop.
A stalled output consumer must not prevent cancellation or the connection deadline.

## Trust boundary

The SSH server and all bytes from it are untrusted until host verification succeeds.
The caller supplies the exact expected public host key through a trusted route.
The package compares this key before it sends authentication material.
An unknown or changed key causes refusal. There is no automatic trust enrollment.

Successful authentication permits only the command that the caller supplied.
It does not establish Horizon project, worker or session authorization.
Later Horizon adapters must preserve those checks before transport creation.
This package does not retry a remote command after an uncertain result.
Closing a connection does not prove that a remote command reversed its effects.

Server output remains untrusted after authentication.
The caller must apply its terminal, transcript and metadata policy to those bytes.
The package bounds retained input and output and reports limits explicitly.
Credentials and secret key material are not serialized, displayed or logged.
Callers must keep their own copies private and clear them when no longer needed.

## Dependencies

`russh` supplies the SSH protocol and established cryptographic implementations.
The selected release is `0.64.1`, with default features disabled and the `ring` backend enabled.
It uses Apache-2.0 and requires Rust 1.89 or later.
Its upstream key dependency includes `ssh-key` version `0.7.0-rc.11`.
That prerequisite is recorded explicitly; it is not described as a stable key-library release.
Tokio supplies asynchronous I/O and bounded channels on the caller's runtime.

The next outcome integrates the established `mosh-client` executable with Rust
SSH bootstrap and a separately installed `mosh-server`. A custom Rust Mosh
implementation is deferred. Do not copy GPL Mosh implementation code or link
the GPL `mosh-rs` package into this library.

## Delivery and qualification

The [SSH procedure](../testing/procedures/remote-session-ssh.md) covers this first package outcome.
Loopback protocol tests and stock OpenSSH interoperability are separate evidence.
Package construction outside the Horizon workspace is a separate check.
Linux, macOS and Windows checks qualify the library only on their tested platforms.

This package alone does not improve the latency of a Horizon terminal.
Session recovery acceptance requires the later cloud attachment and Mosh milestones.
Measure predicted input and confirmed server output separately.
Test loss, sleep, network changes, history catch-up and the same retained session.
RunPod's userspace UDP route needs its own actual provider test.

## References

- [Issue #1368](https://github.com/peters/horizon/issues/1368).
- [Russh API](https://docs.rs/russh/0.64.1/russh/).
- [Mosh protocol and design](https://mosh.org/).
- [RunPod exposed ports](https://docs.runpod.io/pods/configuration/expose-ports).
- [Tailscale userspace networking](https://tailscale.com/docs/concepts/userspace-networking).
