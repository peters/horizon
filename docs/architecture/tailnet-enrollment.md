# Tailnet enrollment trust boundary

## Trusted control path

The Horizon host owns the saved tailnet choice and the auth key.
It sends enrollment data through the cloud's pinned SSH connection.
The root helper, Tailscale binaries and worker image are trusted code.
Workspace code and agents run as UID 10001 without access to the root-only
node state or administrative socket. The auth key uses an anonymous memory file,
not a command argument, environment variable or retained file.

The worker image must report `horizon-tailnet-contract=4`.
The host checks this contract before selected-tailnet allocation, reconnect and
new sessions. Older markers do not qualify. A marker is a declaration by the
image, not proof that a malicious image follows the contract.

## Enrollment guarantee

The saved key and status output are inputs to enrollment.
The helper requests only `--advertise-tags=tag:horizon-worker`.
After `tailscale up`, it reads `Self.Tags` from `tailscale status --json`.
It accepts a node only when this field contains exactly `tag:horizon-worker`.
An unreadable status fails the check. An untagged node or a node with extra tags
is refused and logged out. Each resume checks the retained node once before
the helper accepts it. This includes nodes that an older image enrolled.

The error names `tag:horizon-worker` and the `tagOwners` prerequisite.
The administrator must define the tag in the tailnet policy and permit the key
to assign it. Horizon does not change that policy or create keys.
The helper does not prove that the policy restricts the tag to particular services.

## Limits of the guarantee

The check proves the tags that Tailscale reports at that instant.
There is no continuous tag monitoring after enrollment or resume.
A later tag change does not cause an automatic logout.
The tailnet policy controls reachability throughout the life of the node.
The common tag does not provide isolation between workspaces.

The daemon can start before the one-time status check completes.
This contract does not promise that all proxy listeners stay absent before the
check, or a persistent quarantine after a failed logout.
Logout does not delete the administrative device record.
Cloud deletion does not remove that record in this milestone.

## Compatibility

Existing keys that cannot assign `tag:horizon-worker` fail enrollment with the new helper.
Already-joined untagged clouds log out on resume.
The current quick-start pin contains older markers and refuses a selected tailnet.
Publishing marker 4 and updating the pin are a separate rollout step.
The **None** choice remains compatible with older images.

See [the cloud guide](../cloud-workspaces.md#tailnets-auth-key-mvp) and
[the test procedure](../testing/procedures/tailnet-tagged-enrollment.md).

## Startup storage prerequisite

Before either startup branch writes workspace content, `prepare-workspace`
protects the mount metadata. It opens existing `/workspace` as a directory with
`O_NOFOLLOW`, verifies root ownership and FD/path identity, and seals only an
empty writable root to 0755. It verifies real permissions, identity and emptiness
again. This narrow pre-initialization metadata write removes untrusted write
access; it is not repository initialization, owner approval or permission to
adopt existing state. HORIZON_WORKER_STARTUP is not proof of a fresh volume.

An already safe retained root is unchanged. Retained writable data, other owners,
symlinks, races and ineffective chmod are refused. The strict isolate and private
control-directory checks remain active, with both None and selected tailnets.
Failure diagnostics contain only fixed codes, numeric workspace ownership/mode
and kernel capability masks. Exception messages, environment and file contents
never enter the diagnostic. A runtime that cannot chown/drop privileges or
storage that cannot enforce root-only control permissions remains unsupported.

The published FB image was observed to fail before SSH on one RunPod GPU fixture;
no key had been sent. Offline POSIX/capability cases reproduce that generic failure,
but the actual provider cause is still unverified. Do not infer provider support
from these tests; qualify an image built with this source on first/final heads.
See [the startup storage procedure](../testing/procedures/worker-workspace-storage.md).
