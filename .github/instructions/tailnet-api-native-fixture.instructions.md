---
applyTo: "scripts/tailnet-api-status-fixture/**,crates/horizon-cloud/src/tailnet/api/tests/real_os_store.rs,crates/horizon-cloud/src/tailnet/api/tests/real_os_store/**,docs/testing/procedures/tailnet-api-native-fixture.md"
---

# Native tailnet fixture boundary

This tooling prerequisite builds on API storage79d only. No status product overlay ships.
Baseline qualifies synthetic real-store preparation/cleanup, actual native VNC transport,
a normal host-injected AGENT MCP route and the existing three id/name tailnet items.
It does not require API-status UI/map or the future `tailnets` CLI route.
The explicit `--expect-api-status` mode belongs to later status qualification.

Report concrete failures when:

- Synthetic preparation or cleanup reads a real credential or accepts an unknown catalog ID.
- A fixture mounts provider credentials or host home/runtime/X11/bus sockets.
- Actor or host-instance values are forged instead of supplied by normal agent spawn.
- Private host queues/stamps/caches substitute for timestamped public MCP responses.
- A synthesized image substitutes for actual owned X11 x11vnc bytes.
- Viewer/input, listener, process or container ownership is not exact and bounded.
- An uncertain Docker daemon is treated as absence or broad cleanup deletes unrelated state.
- Baseline invokes the unshipped CLI route or claims absent API status features qualified.
- A skipped test or fixture setup result is described as live/native/interface qualification.

Product code and both skill bundles remain byte-identical to the API foundation.
Real baseline lanes and exact cleanup are mandatory before tooling readiness.
