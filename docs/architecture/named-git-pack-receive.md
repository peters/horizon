# Explicit named private pack receipt

`receive_named_git_base_pack` is a Linux-only opt-in sibling of the existing
receiver. The caller supplies a bounded portable attempt name under an existing
private parent. Expected metadata, limits and parent admission precede input reads.
One successful exclusive mkdir grants ownership; an existing name is never adopted.
Every dispatched claim error retains the attempted locator without proving that it
exists or belongs to this attempt. The caller must establish ownership separately.

The common stream/EOF/digest, isolated strict indexing and exact shallow closure
checks are unchanged. Only final pack/index naming differs: ordinary renames run
once within the exclusively owned reservation using held directory/file identities
and exact child-name checks. Errors may follow completed operations; partial names
are retained, never retried, repaired, overwritten or cleaned. Successful receipt
also passes the existing read-only observer; observation cannot resume a receiver.

Stable ancestry and exclusive same-user control remain caller preconditions. This
is not confinement against a malicious concurrent same-user or privileged writer.
The default receiver, CLI/intake callers and qualified publisher remain unchanged.
There is no implicit fallback, synchronization acknowledgement, cloud operation,
worker enrollment or full checkpoint/recovery claim. Local fixtures and injected
errors test operation ownership and retention, not HPS/NFS compatibility or physical
durability; the complete selected-filesystem receive/publication path still needs
separate live qualification. Blocking storage/input calls have no hard deadline.
