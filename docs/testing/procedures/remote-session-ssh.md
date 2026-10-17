---
procedure: remote-session-ssh
feature: Standalone Rust SSH command transport
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: synthetic test keys only
owner: peters
---

# Remote session SSH test procedure

## 1. Purpose

This procedure tests the independent `remote-session` package from #1368.
It covers pinned host verification, command events, resource bounds and transport shutdown.

## 2. Applicability

- Use the candidate checkout and its locked dependency versions.
- Run library tests on Linux, macOS and Windows.
- Run genuine OpenSSH tests on Unix with an explicitly supplied disposable server.
- Current SSH support is non-PTY command execution with Ed25519 keys and a caller-supplied numeric `SocketAddr`.
- The package does not resolve hostnames, consult saved host trust or authenticate through an agent.
- This procedure does not qualify Horizon terminal attachment, Mosh, RunPod or session recovery.
- Existing Horizon connections and visible UI behavior are unchanged by this package-only outcome.

## 3. Safety

Use synthetic host and client keys in a task-owned directory.
Use loopback addresses for all test servers.
Do not use a saved cloud, a production host or the user's SSH agent.

## 4. Equipment and preconditions

- The candidate Rust toolchain and Cargo lockfile.
- A separate target directory with enough free space.
- An OpenSSH server for the interoperability comparison.
- A task-owned home directory for the example.

## 5. Setup

1. Record the candidate commit and any uncommitted inputs.

   Result: The record identifies the source under test.

2. Record the toolchain, selected features and dependency versions.

   Result: The record identifies the build inputs without secret values.

## 6. Tasks

### RS01 — Library behavior

1. Run the package tests.

   ```bash
   RUSTFLAGS="-D warnings" cargo test --locked -p remote-session
   ```

   Result: Host verification, authentication, event ordering, bounds and cancellation tests pass.

2. Run the package without its default SSH feature.

   ```bash
   RUSTFLAGS="-D warnings" cargo test --locked -p remote-session --no-default-features
   ```

   Result: The package builds without the SSH implementation.

3. Run the package lint checks.

   ```bash
   cargo clippy --locked -p remote-session --all-targets -- -D warnings
   cargo clippy --locked -p remote-session --lib --examples -- -D warnings -D clippy::unwrap_used -D clippy::expect_used
   ```

   Result: The blocking and strict checks pass.

### RS02 — Genuine OpenSSH interoperability

1. Build the example and supply its absolute path with the native `sshd` path.

   ```bash
   cargo build --locked -p remote-session --example ssh_exec
   REMOTE_SESSION_TEST_EXAMPLE="$PWD/target/debug/examples/ssh_exec" \
   REMOTE_SESSION_TEST_SSHD=/usr/sbin/sshd \
     cargo test --locked -p remote-session --test openssh -- --ignored --test-threads=1
   ```

   Result: Each test creates a private directory below the current account home, synthetic keys and a
   foreground loopback-only server for the current test account. Its owned server
   is killed and joined when the fixture ends. No system SSH service is changed.

   For a task-local extracted OpenSSH package whose helpers are outside their
   installed paths, supply its existing prefix instead:

   ```bash
   OPENSSH_PREFIX=/absolute/path/to/extracted/prefix
   REMOTE_SESSION_TEST_EXAMPLE=/absolute/path/to/target/debug/examples/ssh_exec \
   REMOTE_SESSION_TEST_SSHD="$OPENSSH_PREFIX/usr/sbin/sshd" \
   REMOTE_SESSION_TEST_SSHD_SESSION="$OPENSSH_PREFIX/usr/lib/openssh/sshd-session" \
   REMOTE_SESSION_TEST_SSHD_AUTH="$OPENSSH_PREFIX/usr/lib/openssh/sshd-auth" \
     cargo test --locked -p remote-session --test openssh -- --ignored --test-threads=1
   ```

   Result: The fixture uses those exact native helpers through quoted
   `SshdSessionPath` and `SshdAuthPath` configuration. Stock installed OpenSSH
   needs neither override. These are optional test-server paths, not client APIs.
   The supplied server must support the selected directives and its required
   privilege-separation directory must already exist; do not enable a service.

2. Check the six actual outcomes.

   Result: Wrong host key refuses before exec; an unauthorized client key refuses
   authentication; binary stdin survives through EOF with separate stdout/stderr
   and nonzero status; cancellation and deadline retire an accepted command that
   is waiting for input. The example also runs with an empty PATH. Record ignored tests separately from executed tests.

3. Check the client without an external SSH program on PATH.

   Build the `ssh_exec` example first. With a separately retained synthetic
   loopback fixture, record its endpoint, username, private key and public host
   key. Create an empty directory and use the example's absolute executable path:

   ```bash
   /usr/bin/env -i HOME="$fixture/home" PATH="$fixture/empty-bin" \
     "$example" "$endpoint" "$username" "$fixture/client" "$fixture/host.pub" \
     'printf "rust-client-only\n"'
   ```

   Result: The example exits0 and prints exactly `rust-client-only` plus newline
   using the Rust transport. `example` must name the already-built executable,
   not Cargo or a shell wrapper. The `example_connects_without_ssh_programs_on_path` case also performs this check.
   The fixture needs `ssh-keygen` on PATH before it starts the isolated client.
   Do not infer this result merely from source inspection.

Keep server logs private. Do not include key material or remote payloads in public
evidence. Transport shutdown proves neither remote process termination nor rollback.

### RS03 — Independent package

1. Construct and check the package outside the repository workspace.

   ```bash
   cargo package --locked -p remote-session
   ```

   Result: Cargo verifies the packaged source without a Horizon dependency.

2. Examine the package contents.

   Result: The package contains its manifest, licenses, source, examples and required tests.

## 7. Pass criteria

- The expected host key is required before authentication.
- Separate channel events preserve stdout, stderr, EOF and exit status.
- Retained data stays within the declared bounds.
- A stalled consumer does not block cancellation or the connection deadline.
- Independent sessions and the caller's runtime continue while one command stalls.
- Explicit shutdown joins the owned transport.
- The package builds independently on each recorded platform.
- Stock OpenSSH evidence is recorded separately from in-process protocol tests.

## 8. Cleanup

1. Stop and join the test server and client processes.

   Result: No task-owned server or transport remains live.

2. Remove the task-owned synthetic credentials after their last use.

   Result: No test key remains in an active fixture.

## 9. Record of results

Record new, reused and non-applicable results separately in the PR.
Record any missing platform or interoperability result as pending.
Do not describe library success as a complete #1368 or full transport qualification.
