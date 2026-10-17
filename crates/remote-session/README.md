# remote-session

An independent asynchronous SSH command package. It uses the caller's Tokio
runtime and exact public host-key pin. It starts no SSH executable and reads no
Horizon configuration or credential store. This first outcome implements
non-PTY execution; PTY, Mosh, SFTP, agent authentication, host certificates,
known-hosts policy and connection multiplexing are not implemented.

```rust,no_run
use remote_session::{Credentials, ExecRequest, HostKeyPin, execute};
use std::{net::SocketAddr, time::Duration};
use tokio::time::Instant;

# async fn example(endpoint: SocketAddr, trusted_host_key: &str, secret_key: &str)
# -> Result<(), remote_session::Error> {
let pin = HostKeyPin::from_openssh(trusted_host_key)?;
let key = Credentials::from_openssh(secret_key, None)?;
let request = ExecRequest::new(endpoint, "user", "printf hello", pin,
    Instant::now() + Duration::from_secs(30))?;
let mut command = execute(request, key)?;
command.take_input().ok_or(remote_session::Error::Finished)?.eof().await?;
while let Some(event) = command.next_event().await {
    // Handle Stdout/Stderr as untrusted bytes, and EOF/status/close separately.
    let _ = event;
}
let completion = command.completion().await?;
# let _ = completion;
# Ok(()) }
```

The initial key support is Ed25519. A pin is supplied from the caller's trusted
source, never learned from the connection. Comments on public keys are ignored.
Credentials have no Debug or serialization implementation; temporary key and
password copies are zeroized. Callers must clear their original buffers. Commands
are remote shell syntax supplied by the caller, not an argv escaping service.
The package does not log command, key or output contents. Applications must also
control dependency logging and treat remote output as untrusted data.

Each command owns one TCP connection. Eight queued events and eight queued input
chunks apply backpressure; a chunk is at most 32 KiB. The SSH receive window is
128 KiB and the channel queue is eight messages. Default cumulative limits are
1 MiB stdin and 4 MiB each stdout/stderr; callers can lower these or raise each
to at most 64 MiB. The caller-supplied absolute deadline covers connect,
authentication, exec acceptance, input/output and remote close. Shutdown may take
up to two additional seconds to join the upstream session. ShutdownTimeout
reports failure to establish retirement, rather than claiming it succeeded.

Take CommandInput to write concurrently with event consumption. Dropping the
input owner sends EOF after queued input. RemoteEof is observed separately from
exit status; output is drained until remote channel close. Missing exit status
or signal is an error, never an assumed exit zero. Drain next_event until None,
then await completion. Awaiting completion while retaining a full event queue
can wait until the command deadline. Retrying completion after cancelling its
wait is supported.

A cloned cancellation handle physically shuts down the same original TCP
connection independently of queue backpressure. shutdown cancels and awaits
retirement, normally returning Cancelled. Drop also cancels, but cannot await;
keep the caller's runtime running and use explicit shutdown when retirement
proof is needed. Transport closure does not prove the remote process was killed.
Commands on other connections remain independent.

Run the example with a numeric socket address and an independently trusted public
host-key file:

```sh
cargo run --example ssh_exec -- 127.0.0.1:2222 user ./id_ed25519 ./host.pub 'printf hello'
```

The example closes stdin immediately and forwards binary stdout/stderr separately.
Its key is an unencrypted test/user-supplied file; the library itself accepts
in-memory encrypted keys and a caller-supplied password.

The package has explicit manifest metadata and dependency versions so its
packaged source can build outside this repository. Default feature ssh can be
disabled. There is no Mosh feature or implementation yet. russh is pinned to
0.64.1 under Apache-2.0, with defaults disabled and ring enabled. Its exact
ssh-key 0.7.0-rc.11 transitive dependency is a prerelease. The package's own
MSRV is Rust 1.95; dependency resolution and supported-platform interoperability
must be checked on the candidate, not inferred from russh's declared MSRV1.89.

License: MIT OR Apache-2.0. Tests use task-owned loopback servers; the stock
OpenSSH interoperability fixture is separate from the Rust server regressions.
No cloud deployment, network-policy mutation or credential-store access is
required for these tests.
