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

For quick start, the host trusts the published workflow report only for the pinned
immutable digest and exact built-in capabilities. Other images or tool selections
keep the local image check. Publication and marker checks do not prove that a
provider mount enforces private modes or qualify an actual enrollment.

## Development CLI selection

The development `cloud_deploy select-tailnet CATALOG_ROOT CLOUD_ID TAILNET_ID`
command accepts caller-supplied paths and IDs. It is an authorized host operation,
not an agent membership or native approval check.
It delegates to the existing locked `tailnet::select` function.
The original catalog supplies the saved ID; the cloud stays under that catalog root.
Pending operations and allocated clouds retain their existing selection guards.
Missing, extra or empty-ID arguments refuse before path creation.
Unsupported hosts refuse before cloud state writes.

Selection reads no secret, changes no catalog and starts no remote operation.
It proves no key validity, enrollment or reachability.
Later normal deployment and recorded-node cleanup use the same saved ID and original catalog.
Read [the selection procedure](../testing/procedures/cloud-tailnet-selection.md).

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
The quick-start pin contains marker 4. New selected-tailnet clouds still require
storage that preserves private root-owned modes. Existing clouds keep their
recorded image until an explicit rebuild. The tested RunPod persistent storage
does not enforce these modes and is unsupported; the helper refuses it rather
than weakening isolation. CPU image qualification does not qualify GPU workloads.
The **None** choice remains compatible with older images.

See the [marker-4 rollout procedure](../testing/procedures/tailnet-marker4-rollout.md)
for publication, provider evidence and unsupported-lane reporting.

See [the cloud guide](../cloud-workspaces.md#tailnets-auth-key-mvp) and
[the test procedure](../testing/procedures/tailnet-tagged-enrollment.md).
