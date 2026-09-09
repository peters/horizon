# Worker pack receipt and observation

`horizon-repository receive-pack` streams one explicitly supplied exact-base Git
pack into private staging, then performs qualified no-overwrite publication.
`pack-status` inspects one explicit candidate without writing. These Linux commands
reuse the [receiver](remote-repository-pack-receipt.md),
[observer](remote-repository-pack-receipt.md#read-only-reopening) and
[publisher](remote-repository-pack-publication.md). They are not source-export
approval, authenticated transport, setup/task admission or workspace readiness.
Nothing invokes them automatically; existing commands remain compatible.

## Requests

For `receive-pack`, stdin is a four-byte unsigned little-endian header length
(1–32,768), exactly that many UTF-8 JSON bytes, exactly `pack.encoded_bytes` pack
bytes, then EOF. The header has only these fields:

```json
{
  "version": 1,
  "parent": "/retained/explicit-private-parent",
  "destination": "base-pack",
  "pack": {
    "base_commit": "1111111111111111111111111111111111111111",
    "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "encoded_bytes": 123
  }
}
```

The identifiers above are illustrative, not a usable fixture. `base_commit` is
an exact nonzero 40-character SHA-1 commit identity; `sha256` is the expected
64-character encoded-pack SHA-256. Shared core types normalize either hex case to
lowercase. The core default encoded limit is 256 MiB, with a minimum of 32 bytes.
Existing native decoding, object-count and per-process resource limits also apply.
The binary does not buffer the complete pack or accept archives/thin packs.

`parent` must be an explicit existing absolute non-root path, at most 4,096 UTF-8
bytes, without NUL or parent traversal. `destination` uses the shared portable
single-component naming policy, at most 255 bytes. Unknown/duplicate fields,
unsupported versions, malformed identities, paths and headers fail before storage
access. Core length limits fail before reservation. Payload truncation, trailing
bytes, digest mismatch or decoding failure can leave unconfirmed private staging.

For `pack-status`, stdin is ordinary JSON, at most 32,768 bytes plus EOF:

```json
{
  "version": 1,
  "path": "/retained/explicit-private-parent/base-pack",
  "pack": {
    "base_commit": "1111111111111111111111111111111111111111",
    "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "encoded_bytes": 123
  }
}
```

Only those fields are accepted. `path` follows the same lexical path rules. Status
revalidates the existing fixed layout, pack bytes and exact-base object closure.
It does not create, synchronize, repair or distinguish missing from unsafe,
unreadable or invalid data. Observation is not a new publication acknowledgement.

## Responses and retained data

Both commands return bounded JSON plus newline, at most 128 KiB:
`version`, `status`, `pack`, `retained` and `reason`. Only success/observation has
non-null `pack`, containing `path`, `objects_directory`, the verified request
`identity` and decoded `objects` count. That object directory still needs existing
namespace/seed/setup verification before use. Success has null `retained`/`reason`.
Failure has null `pack` and a static redacted reason; candidate paths appear only
in typed `retained.source` and `retained.destination` fields.

| Status | Exit | Meaning / retained candidate |
| --- | --- | --- |
| `acknowledged` | 0 | This invocation completed qualified publication and final confirmation. |
| `observed` | 0 | Existing matching data was read and verified, with no write. |
| `rejected` | 2 | Invalid request; execution did not begin. |
| `unsupported` | 1 | Non-Linux platform; no storage operation. |
| `error` | 1 | Receipt failed without a returned reservation, or observation failed. No path receipt is available; this is not cleanup/replay authority. |
| `receive_unconfirmed` | 1 | Partial or unverified data may remain at `source`. |
| `unpublished` | 1 | Publication did not rename the verified input; retain `source`. |
| `published_unsynchronized` | 1 | Rename succeeded, final confirmation failed; retain `destination`. |
| `rename_unconfirmed` | 1 | Rename outcome is uncertain; retain and inspect both candidates. |

An existing destination never bypasses payload validation or fabricates success.
A repeated receive can leave another unpublished private source; it is not an
idempotent status request. No failure triggers retransmission, cleanup or setup.

Exit 3 means a response could not be fully written/flushed, including after a
successful publication. Persist the intended destination and expected identity
**before** sending input. After output loss, use status on that same destination.
Error/missing output never authorizes another receive or deletion. If failure
occurred before publication, random private staging may remain under the nominated
parent without a received path; explicit inventory/cleanup is separate work. A
later successful observation does not prove a prior failed synchronization succeeded.

## Preconditions and proof boundary

The caller authorizes the nominated paths and keeps their private ownership,
contents, ancestry and mounts stable/exclusive. Publication requires qualified
journaled ext4 with barriers and trusted readable proc/sysfs metadata; container
overlay storage fails closed. No fallback, ownership repair or permission change
is attempted. Acknowledgement is not proof of power-loss survival/cloud durability.

These commands are synchronous. Blocking stdin/EOF and filesystem calls are the
caller's latency responsibility; native child bounds are not an end-to-end deadline.
The future authenticated controller must impose admission, transport and capacity
budgets without treating disconnect as cancellation or replay permission.
No keys, credentials, provider resources, client transport, task supervision,
checkpoints or image release are added. Closing local views still only detaches.

Run actual command proof against a locally built image and trusted ext4 fixture
parent with `python3 -B containers/remote-worker/test_pack_receive_image.py
--docker-host unix:///path/to/docker.sock --image <immutable-image-id>`. The harness
uses only synthetic data and no network, checks small/65 MiB-plus receipt and
fresh-container observation, failure retention, output loss and materialization,
then removes only its owned fixtures/containers. Images remain. This is local
mechanism proof, not authenticated transfer or cloud/PC-off acceptance.
