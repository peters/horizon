---
procedure: worker-workspace-storage
feature: Worker startup storage protection and nonsecret prerequisite diagnostics
platforms: [linux]
cost: none
owner: peters
---

# Worker startup storage prerequisite

Run on an isolated checkout and owned offline containers; use no credentials,
real tailnet, paid provider or host daemon. The empty-root sealing is a narrow
metadata prerequisite before workspace content initialization. It grants no
workspace ownership and does not adopt persistent state.

Run `python3 -m unittest -v test_workspace_seal.py test_tailnet.py` from
`examples/cloud-worker`. Run the same sources as root in an owned read-only,
`--network=none` container, with private tmpfs `/run`, executable `/tmp` for shell fixture helpers, and `/workspace`.
Record source hashes, exact pinned image, flags, command and logs.

Required cases:

1. Empty root-owned 0777 storage becomes 0755; inode/owner stay unchanged and no
   directory or file is created by `prepare-workspace`. It then supports actual
   isolate and None resume, without a Tailscale daemon.
2. Safe root-owned retained storage is unchanged. Retained 0777 storage refuses
   without chmod or reading/adopting private state; a prior selection stays intact.
3. Other UID, symlink and regular-file roots refuse without chmod/following them.
4. Ignored or failed chmod, replacement between open/snapshot, replacement during
   sealing and a concurrent directory entry all refuse. Simulated ignored chmod
   is not a real FUSE qualification result.
5. Startup failure stops **both** signed-allocation and legacy branches before
   allocation/SSH/content work. An actual shell test checks exit and command log;
   source ordering checks that mount protection precedes either branch.
6. The signed-allocation path sees the sealed empty root before its runtime-only
   preparation. A deliberate runtime stub failure proves there is still no
   workspace content or agent marker at that point; no SSH starts.
7. Overlay candidate helpers onto the exact published base read-only and use
   actual root Docker default capabilities. Root-owned 0755/0777 fresh volumes
   pass, while other UID and retained writable data refuse. Drop CHOWN and use a
   minimal capability profile to prove isolation refusal has fixed stage/capability
   evidence, and cannot become ready merely because the mount was sealed.
8. Private-control mode enforcement stays strict. A simulated private chmod no-op
   refuses; a root startup error must distinguish prerequisite failure from tag
   failure. Tag refusals still name tag:horizon-worker/tagOwners; unrelated errors
   remain generic. Synthetic key text in exception/env never appears in evidence.

The exact FB image fails in several offline cases with the generic message seen
on one RunPod GPU attempt. This identifies reproducible classes, not that live
provider's cause. Official RunPod storage docs distinguish persistent Pod volume
disk from network volumes; do not infer FUSE semantics from a missing network
volume ID or conflate those storage types.

Before publication/merge, root runs the required local matrix, image recipe
checks, independent review and exact source-to-image report checks. Real provider
first/final-head qualification is separate and still required. On a failed boot,
read only sanitized provider logs from exact task-owned IDs; the fixed failure
line gives a stage and numeric metadata without credentials. Stop after an
unsupported prerequisite, preserve uncertain state and clean only owned resources.
Do not bypass isolation or start SSH to obtain diagnostics. A diagnostic build
must itself pass review and be published/qualified before a new live attempt.
