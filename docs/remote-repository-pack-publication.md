# Qualified private pack publication

`receive::publication::publish_sibling_git_pack` explicitly publishes one already
received exact-base pack to a fresh sibling. It reuses
[pack receipt and read-only observation](remote-repository-pack-receipt.md);
neither receipt nor observation implicitly publishes input or starts setup.

## Contract

The caller supplies a `ReceivedGitPack`, one fresh single-component sibling name,
`PackReceiveLimits` and cancellation. The existing private parent and received
tree must remain exclusively controlled and unchanged through verification,
publication and later consumption. Parent/root identities, ownership, modes and
same-device placement are checked; symlink ancestry and unsafe nodes fail closed.

Linux journaled ext4 with barriers is required, using the existing storage
qualifier and trusted kernel metadata. Unsupported or unreadable qualification
fails without synchronization or rename. There is no copy, alternate-filesystem,
global-sync or remote-storage fallback. Mount configuration must remain unchanged.

The operation pins the fixed received layout before rechecking encoded identity,
native index and complete shallow commit closure. It synchronizes eight files,
then eleven directories in reverse parent order. Bindings are rechecked during
synchronization. A descriptor-relative `RENAME_NOREPLACE` transfers the directory
to the explicit sibling; an existing file, directory or link is never replaced.

After rename, the published root and parent are synchronized and checked again.
The root keeps its inode; its rename-induced change time is permitted. Every
non-root node must retain its previously verified metadata fingerprint. Even a
same-content inode replacement after file synchronization prevents acknowledgement.
No new named metadata, data writes or permission changes are introduced.

## Result and failure ownership

`PublishedGitPack::pack()` exposes the underlying verified input for existing
namespace and checkout consumers. A successful result acknowledges the completed
file/directory synchronization and binding checks on healthy qualified storage.

| Result | Retained identity | Meaning |
| --- | --- | --- |
| Success | Destination | Rename and final synchronization/verification acknowledged |
| `Unpublished` | Source receipt | This publication did not confirm a rename |
| `PublishedUnsynchronized` | Destination receipt | Rename succeeded; later synchronization, verification or cancellation prevented final confirmation |
| `RenameUnconfirmed` | Source receipt and destination candidate | The rename result is uncertain; inspect both names |

The destination receipt is updated immediately after a successful rename, before
any later fallible operation. Unknown rename results do not assume the source
still has its original name. No variant authorizes rollback, repair, retry,
overwrite, cleanup or task execution. Dropping any receipt retains data. Private
paths and contents are excluded from error formatting.

Use `observe_git_base_pack` with an explicit candidate path and expected identity
to inspect current data after a lost result. Successful observation proves current
private input, not that a previous synchronization was acknowledged. It does not
reissue a publication acknowledgement, source approval or setup grant.

## Limits and evidence

The same encoded/native limits as receipt apply. Run off the UI thread:
cancellation is checked between operations and cannot interrupt blocking storage
calls. Publication is immutable by protocol, not protected against hostile
same-user or privileged mutation. This is not a snapshot, proof of power-loss
survival, proof against earlier writeback errors, provider durability or PC-off
acceptance. Source approval, namespace policy and setup admission remain separate.

Native tests cover real qualified publication, reopening and checkout consumption;
all 21 synchronization boundaries; pre/post-rename cancellation and binding changes;
same-content inode substitution; existing/invalid names; unsafe input and failed
qualification; both uncertain-rename outcomes; and competing same-name publishers.
Snapshots assert retained names, bytes and inode metadata. The real qualification
test explicitly reports unavailable filesystem capability rather than substituting
a different filesystem or claiming durability from an injected qualifier.

Worker receive/status commands, pinned transport, client/UI integration and cloud
acceptance remain separate. Closing local views never implicitly stops or deletes
a persistent environment.
