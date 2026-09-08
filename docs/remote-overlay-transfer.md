# Worker overlay receipt

The worker's `horizon-repository` helper accepts one explicitly supplied canonical
overlay bundle into an existing private bundle store. This is a transfer primitive,
not permission to capture/export local files, transfer Git objects, start repository
setup/tasks, select a provider or mark a workspace ready. Existing commands remain
compatible; nothing invokes these commands automatically.

## Receive

```sh
horizon-repository receive-overlay < framed-request.bin
```

Stdin contains exactly these ordered parts:

1. A four-byte unsigned little-endian JSON-header length, from 1 through 32,768.
2. Exactly that many UTF-8 JSON bytes with the fields below.
3. Exactly `encoded_bytes` bytes in the existing version-1 overlay bundle codec.
4. EOF, with no trailing bytes.

| Header field | Contract |
| --- | --- |
| `version` | Integer `1`. |
| `bundle_store` | Explicit existing absolute non-root directory; at most 4,096 UTF-8 path bytes, no parent traversal or NUL. |
| `bundle_manifest` | Expected canonical metadata SHA-256, 64 hexadecimal characters, normalized to lowercase. |
| `encoded_bytes` | Positive integer, at most `codec::MAX_ENCODED_BUNDLE_BYTES`. |

Unknown/duplicate fields, unsupported versions, oversized framing, truncated or
trailing input, invalid canonical encoding, bad payload hashes and manifest mismatch
are rejected before opening storage. The bundle codec retains its 64 MiB per-file,
128 MiB aggregate payload and separate metadata/entry limits. This does not add a
second bundle encoding, accept archives, interpret paths as shell arguments or
copy arbitrary files from the worker filesystem.

The root must satisfy the existing owned `0700`, no-symlink and confined bundle
store contract. The caller explicitly authorizes this target, keeps its selection
and ancestry trusted/stable, and owns capacity and retention policy. No directory
creation, permission repair, alternate target, overwrite or named-data cleanup is
implicit. A store is selected for bundle receipt, not inferred from a setup claim.

The complete bounded input buffer is dropped before storage re-encodes the verified
bundle. An identical retry can hold the verified payload and two encoded copies,
as documented by `RepositoryBundleStore::put`. Framing limits are not a hard RSS or
wall-clock deadline; missing EOF and blocked storage can still block the caller.
The authenticated transport/controller must impose its own admission and budgets.

## Observe a lost acknowledgement

```sh
horizon-repository overlay-status < request.json
```

This command reads a bounded ordinary JSON object containing only `version`,
`bundle_store` and `bundle_manifest`, followed by EOF. It opens the nominated store
and validates the existing digest-named record and all bundle contents without
publishing, synchronizing, creating or replacing anything. An unavailable/unsafe
root or invalid existing record is an error, never a `missing` observation.

Both commands return complete JSON plus newline, at most 1 KiB, with `version`,
`status`, `bundle_manifest` and `reason`. Only acknowledged/observed responses have
a non-null manifest. Reasons are bounded static diagnostics, not paths or payloads.

| Status | Exit | Meaning |
| --- | --- | --- |
| `acknowledged` | 0 | `put` completed its no-replace publication or identical-record verification and file/directory synchronization. |
| `observed` | 0 | A complete matching record was read and verified; this does not acknowledge a new synchronization. |
| `missing` | 4 | A valid opened store had no record for the requested digest. |
| `rejected` | 2 | Request framing or bundle validation failed before storage access. |
| `error` | 1 | Storage could not be safely opened or existing data could not be observed. |
| `write_unconfirmed` | 1 | Publication was not acknowledged; a complete existing or newly published record may remain. |

Exit 3 means the response could not be completely written/flushed. It does not
undo publication or retry any operation. Re-observe the same store/digest after
lost output. Explicitly resending the same complete verified bundle can recheck and
resynchronize the existing immutable record; it cannot overwrite a conflicting or
unsafe node. This storage retry is not permission to replay retained setup/tasks.

## Storage and product limits

This reuses the bundle store's anonymous-file publication and synchronization
contract. It does **not** add the retained-setup journaled-ext4 qualifier to that
store or strengthen storage guarantees. Input staging, qualified setup output,
remote checkpoints, replication and cloud/PC-off acceptance remain separate gates.
Read-only observation cannot prove a prior failed synchronization succeeded.

No client wiring, source selection/approval UI, Git base closure transport,
worker-loss recovery, remote backup scheduling, provider lifetime or image release
is included. All commands preserve the rule that closing a local view never
implicitly stops/deletes a persistent environment.
