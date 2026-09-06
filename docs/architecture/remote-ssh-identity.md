# Retained remote SSH client identity

Remote worker connections use a dedicated per-allocation client key, not a user's
general SSH identity or repository credentials. The private key belongs to a
local private identity store independent of board/session files. Closing handles,
panels or Horizon does not remove it or perform any provider operation.

The current implementation supports Linux clients with a trusted local
POSIX-permission filesystem and OpenSSH `ssh-keygen` on PATH. Arbitrary network
shares are outside this boundary. Other platforms explicitly fail before writes:
macOS inherited ACLs can grant access independently of the checked mode bits,
and Windows requires its own ACL-aware implementation. Both need dedicated
permission checks and current-head platform smoke before enabling key storage.
This boundary is not a completed cross-platform remote-workspace feature.

## Ordering and recovery

For a new, unclaimed allocation, `prepare_new` retains one key under its exact
workflow/job UUIDs. An interrupted setup may reuse that existing candidate.
Independent processes generate in separate owner-only staging directories and
publish without overwrite; a loser reloads the winning complete key. Files and
directory entries are synchronized before the candidate is returned. An error
after publication may leave a valid retained candidate; callers must not delete it
as failure cleanup or create a second allocation merely to retry.

Only after durable key retention may the coordinator reserve the public identity
in the allocation store, then consume the one-shot provider creation grant.
`prepare_new` is not a recovery path for a reserved or claimed allocation.
`recover` only loads an existing key and derives its public key with OpenSSH,
comparing it exactly to the saved request. Missing, invalid, insecure or mismatched
keys fail visibly without replacement, key rotation or a provider call. The
coordinator must preserve the remote worker and surface recovery instructions.

## Private storage boundary

Every existing ancestor of the configured Horizon home is checked from the root
before any write. Ancestors must be directories owned by the effective user or
root, without symlinks or parent traversal. Shared writable ancestors require the
sticky bit; owned children beneath them cannot be renamed by other users. The
home itself must belong to the effective user and must not be writable by others.
The dedicated identity directory and private regular files must also belong to
that user and be owner-only (`0700` and `0600` when created).
Keys are unencrypted OpenSSH files protected by filesystem permissions; an
encrypted vault or OS keychain is not claimed by this implementation.
Symlinks, non-regular files, broad permissions and oversized/empty files are
rejected, not silently repaired. Same-user filesystem tampering is outside this
permission boundary, as with other local private stores. The filesystem must
support durable same-filesystem no-overwrite publication.

Private bytes are never serialized or read into application strings. Public-key
derivation has bounded output and a five-second process deadline, with prompts
disabled and diagnostics discarded. Errors and identity debug formatting omit
paths, key bytes and subprocess output. Key utility processes are task-owned and
reaped on completion or error; remote processes are never affected.

The existing public-key validator and atomic file publication dependency are
reused. A Linux-only direct `rustix` dependency provides the safe effective-user-ID
API without unsafe code; the version already exists in the dependency graph and
was verified as the latest stable on crates.io. No cryptographic format
implementation, credential upload, default configuration or automatic key
deletion is introduced. Explicit key
retirement must remain separate from ordinary disconnection and cannot precede
verified remote cleanup and the required user decision.

OpenSSH option behavior follows its [key utility manual](https://man.openbsd.org/ssh-keygen.1).
Atomic publication uses the existing dependency's no-overwrite move and directory
synchronization; the Rust file synchronization contract is documented by
[File::sync_all](https://doc.rust-lang.org/std/fs/struct.File.html#method.sync_all).
