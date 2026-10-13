---
applyTo: "examples/cloud-worker/horizon-worker-tailnet,examples/cloud-worker/horizon-worker-check,examples/cloud-worker/test_tailnet.py,crates/horizon-core/src/cloud_runtime/worker_contract*,crates/horizon-core/src/cloud_runtime/worker_contract/**,crates/horizon-core/src/cloud_runtime/tailnet*,crates/horizon-core/src/cloud_runtime/tailnet/**,crates/horizon-core/src/cloud_runtime/deployment/**,crates/horizon-core/src/cloud_runtime/ssh.rs,crates/horizon-core/src/cloud_runtime/repository/launch/quick_start.rs"
---

# Tailnet enrollment review boundary

Read `docs/architecture/tailnet-enrollment.md` for the contract and its limits.
The host, root worker helper, Tailscale binaries and worker image are trusted.
Workspace code and agents are untrusted toward the private key and node state.

Report these failures:

- A selected tailnet accepts an image that lacks `horizon-tailnet-contract=4`
  before allocation, reconnect or a new session.
- Enrollment omits `--advertise-tags=tag:horizon-worker`.
- Enrollment returns ready without a successful `Self.Tags` check after `up`.
- Resume accepts a retained node without its one-time `Self.Tags` check.
- A tag check accepts missing, malformed, untagged or extra-tag status.
- A refused node does not attempt logout, or the error omits the tag or `tagOwners`.
- An auth key reaches command arguments, logs, the agent environment or retained files.
- An agent can read private node state or use the administrative socket.
- The quick-start pin claims marker 4 without the published image report.

These limits are deliberate in this milestone:

- No continuous tag monitoring, watcher or persistent quarantine.
- No separate tag for each workspace or automatic tailnet policy edits.
- No promise that proxy listeners stay absent before the tag check.
- No administrative node deletion or persistent revocation marker after failed logout.
- An image's contract declaration does not prove that a malicious image is honest.

Do not request these deferred features as fixes to the enrollment milestone.
