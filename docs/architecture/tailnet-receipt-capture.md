# Tailnet receipt capture

## Deployment and reconnect

Selected tailnet deployments and reconnects capture and persist the worker device ID.
The shared core controller performs the same capture for UI, CLI and MCP lifecycle calls.
Tailnet None keeps its existing behavior and records no tailnet device.
The stock marker-4 worker paths are sufficient; no new worker image capability is added.
A candidate still needs its full matrix and real-worker qualification before merge.

## Trusted authority

Capture trusts the host record, pinned root SSH connection and worker image.
The marker-4 image supplies the official Tailscale CLI, private daemon and control lock.
The current image copies Tailscale 1.102.5 to `/usr/local/bin/tailscale`.
The script calls its fixed private socket with `status --json --peers=false`.
Capture adds no worker-helper capability and does not modify the worker image.

Custom images must preserve these paths and ownership rules.
The state and runtime directories have root ownership and mode 0700.
Their parent directories have root ownership without group or world write access.
The selection file has root ownership and mode 0600.
The existing control lock is a regular root file without group or world write access.
The socket has root ownership inside the private runtime directory.
The CLI and its parent directories have root ownership without group or world write access.
Path traversal refuses symlinks; opened descriptors bind the files used by the script.

The root script can read private daemon metadata.
It sends only a version, saved local tailnet ID and `Self.ID` to the host.
It suppresses raw status, peer records, profiles and CLI errors.
It accepts only Running status and the sole tag `tag:horizon-worker`.
It never substitutes a peer ID, numeric `NodeID`, address or device name.
The saved local ID does not prove the remote API account identity.
A compromised root or image can fabricate the reported ID.

## Bounds and ordering

The script opens the existing control lock without creating it.
An exclusive lock covers the selection check and status projection.
The script reads at most 64 KiB plus one byte before it parses status.
It rejects duplicate JSON fields and malformed status.
The lock and child share a ten-second deadline.
The script kills and reaps its child group on failure or interruption.
The host private exchange has a fifteen-second limit and supports cancellation.
The host requires a strict projected reply of at most 1024 bytes.

Before configure, the host saves an attempt for the actual bound worker.
A failed attempt save prevents enrollment.
After configure, the host records the projected ID in a new complete deployment snapshot.
It saves that snapshot before the optional device-name read, source transfer or sessions.
A later display failure cannot erase the saved receipt.
An exact repeated receipt does not add another entry.
Different IDs and prior unresolved attempts remain intact.

An interrupted enrollment can create a node before the host captures its ID.
The host retains the unresolved attempt and never creates an ID from a display name.
Missing paths, unsafe ownership or malformed authority refuse successful capture.
Capture failure leaves evidence for manual reconciliation.
Capture sends no API credential to the worker and removes no tailnet node.

See [the capture procedure](../testing/procedures/tailnet-receipt-capture.md).

The host verifies the bound worker against the existing WorkerSpec, then checks the capture connection's provider endpoint, pinned worker alias and known-hosts file in the owning cloud root. It retains the provider allocation request ID alongside the exact saved tailnet/provider/worker IDs. The worker receives only the selected saved tailnet ID; it receives no API token/OAuth secret. The owning locked cloud record scopes persistence; it is not a new workspace grant or deletion authority.

Tailscale status Self.ID is a stable node ID. The official API client recommends NodeID for device Get/Delete instead of the legacy numeric ID: https://github.com/tailscale/tailscale-client-go-v2/blob/main/devices.go. Names, MagicDNS labels, tags and first-peer IDs do not substitute for it. The root controller reads the userspace daemon; UID10001 agents are deliberately refused before any CLI query. The projection is trusted-root/daemon observation, not cryptographic or atomic control-plane proof.

First-reviewable and final heads require real selected marker-4 worker qualification, including exact captured Self.ID and stop/resume. No API editor, new worker image or native-session test framework is required for capture. Capture adds no API deletion or new public MCP operation. Unknown capture attempts stay recorded. Typed manual cleanup reasons distinguish missing identity, missing desktop API access and offline access for the later node-cleanup integration; capture does not emit a new public cleanup status.
