# Explicit named bundle publication

`RepositoryBundleStore::open_named` selects a separate, explicit on-disk layout
in an existing owned `0700` root. The default `open` and anonymous-file `put`
behavior remain unchanged; there is no fallback, migration or format detection.

The named layout uses `<digest>/record.hzov`. Only a successful exclusive mkdir
grants ownership of a new digest slot. Its sole creator writes a `0600` `pending`
file, synchronizes it, renames it within the slot, synchronizes the record and
both directories, and re-reads exact bytes and inode identity before success.
The encoded bundle and digest validation are the existing bounded codec.

An existing slot never grants write ownership. A complete identical record can
only be read, compared and synchronized again. A partial claim is a conflict,
not absence and not a reusable lock; it and its bytes remain intact. No cleanup
or overwrite is automatic. Concurrent writers may receive a conflict while a
claim is incomplete, and may explicitly verify the completed winner later.

Ambiguous mkdir errors do not confer ownership. Any rename error is reported
without retrying that mutation: a network filesystem may have completed the
rename despite its error reply. A later explicit operation can acknowledge only
the fully verified and re-synchronized completed record. Symlinked ancestors,
cross-filesystem slot traversal, wrong ownership/private modes and hardlinked
records are refused. Stable trusted worker ownership remains a precondition;
this is not a defense against a malicious owner changing their own directory.

The named path avoids `O_TMPFILE`, anonymous `/proc/self/fd` linking and
`RENAME_NOREPLACE`. Existing safe reads still use Linux descriptor confinement
and reopen held regular files through procfs. Named create/rename/fsync support
must be verified on the selected filesystem. These API and local test results
are **not** RunPod HPS qualification, physical power-loss durability proof,
independent replication or provider retention/replacement acceptance. No PC
transfer, provider operation, capture scheduler or checkpoint watermark is added.
