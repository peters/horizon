# Pinned Unix SSH attachment

The optional Core feature `one-service-gateway` adds an internal SSH attachment
for the exact endpoint engine. The feature stays off by default.
No UI, CLI or MCP operation starts this attachment.
This component does not complete issue #1335 M1-1.

## Inputs and authority

Endpoint text, DNS, routes, client metadata and worker replies are untrusted.
The existing engine retains its LAN and kernel-tailnet checks.
It compares the original binding before start, at admission and during source checks.
It connects only to the captured literal address and TCP port.

A trusted caller must authorize the recipient and exact endpoint before start.
`Gateway::attach` consumes a private gateway and the original pinned `Connection`.
Neither object proves owner approval, saved membership or durable intent.
A saved path, receipt, decoded record or worker Ready reply supplies none of that authority.
This component adds no owner controller, allocation alias or admission transition.

## Private ingress and SSH

`Gateway::start` retains the existing TCP engine on every supported platform.
On Unix, explicit `Gateway::start_private` binds a Unix socket in a fresh mode 0700 directory.
Its socket has mode 0600. The directory remains owned until the gateway retires.
The fixed `/tmp` parent must be root-owned with mode 01777 and cannot be a symlink.
An unavailable parent causes refusal; caller paths and `TMPDIR` cannot replace it.

SSH forwards the existing nonce-specific worker socket to this exact private path.
The numeric proxy port remains reserved only to preserve the receipt shape.
No TCP accept loop serves that numeric port.
A stale Unix path cannot target a successor at a reused numeric port.
The ordinary Local Network Bridge retains its current loopback TCP listener.
Attachment refuses an ordinary TCP gateway; it has no TCP fallback.
On non-Unix hosts, private start and attachment return UnsupportedSource.

The attachment uses `Connection::pinned_args` and the unchanged prepare and hold commands.
Strict host-key checks, the original key path and original known-host path remain required.
SSH disables agent and X11 forwarding and requires successful remote forwarding.
No provider or Tailscale API credential is read, copied or sent to the peer.
The attachment refuses discovery requests and advertises no discovery sources.
The helper's descriptive subnet does not widen the exact endpoint gate.

## Lifetime and status

The existing supervisor owns preparation, Ready validation, heartbeat and retry.
The attachment checks the gateway's current state before retry and during its held session.
Source revocation stops the original helper and prevents another SSH attempt.
An active binding can retry a lost connection under the existing bounded backoff.
Preparation keeps its existing 40-second deadline and checks the source before hold.
Exact close also cancels a pending preparation through the shared cancellation handle.

Attachment input uses a nonblocking Unix pipe.
A child that stops reading cannot block Hello, heartbeat or owned Drop.
The ordinary bridge retains its existing input policy.
Preparation, Ready, command output, discovery queues and relay bounds remain unchanged.

Close accepts only the original operation, generation and receipt port.
A stale receipt changes neither the gateway nor its cancellation state.
Exact close revokes the physical source before supervisor retirement.
Drop closes the source and joins its owned supervisor.
Private socket custody ends with the original gateway.

Cached status separates the gateway state from the actual helper state.
Only a usable reply from the unchanged helper supplies its loopback proxy address.
Local Active and helper Ready do not prove target reachability or public exposure.
No state from this component recreates access after application restart.

## Qualification and remaining delivery

Linux private-fixture tests cover original SSH arguments, private path isolation, stale close,
source revocation, bounded input and relay bytes after request EOF.
The opt-in Linux fixtures require the original task allocation, Store, settings,
SSH key metadata and known-host pins. They test actual LAN and tailnet targets separately.
The fixture does not read or hash private key bytes.
These private fixture modules run only on Linux; they have no conditional Linux skip.
Existing Engine tests retain their platform coverage and TCP behavior, including macOS.
Private attachment on macOS has no qualification claim; a symlink at `/tmp` causes refusal.

Use the [attachment procedure](../testing/procedures/pinned-unix-ssh-attachment.md)
on the first reviewable source and final source before the PR can qualify.
Old combined-source evidence cannot qualify a different executable.
Keep ignored, blocked and actual results separate.

M1 still requires the trusted owner controller, durable dispatch, current saved
membership, UI/CLI/MCP parity and actual native owner evidence.
This attachment adds no provider operation, enrollment, policy mutation, public
ingress, worker protocol extension or expiry rule.
M1 sharing has no expiry; M2 retains its separate exposure lifetime.
