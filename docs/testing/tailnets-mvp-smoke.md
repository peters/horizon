# Tailnets MVP: local qualification, 3 October 2026

Auth keys and cloud panels only. OAuth, API administration and Remote Hosts are
follow-ups to [#1166](https://github.com/peters/horizon/issues/1166).

## Candidate

- Worktree: `issue-1166-cloud-tailnets`, based on `08415419f`.
- Frozen Linux UI SHA256: `14326c3ea03de1481eccc6cd2faacc8cbbb4ddf8b6b706849d82032e8ccce023`.
  Both actual application children matched this hash.
- Final minimal worker digest: `sha256:1900da40b7a35805e8bec78e63bac83fb70354cddfa88cb2a00eca2133680efc`.
  The resumed live worker's networking helper matched the final source.
- Ubuntu 24.04, approximately 538 MB image, no agents, browsers or desktop.
  Ubuntu's libc matches the worker helper requirements. The provider used a
  2-vCPU/4-GB server, its minimum 40-GB root disk, and a 10-GB persistent volume.

## Measured evidence

| Lane | Result |
| --- | --- |
| Workspace regression | 4,686 passed, 36 ignored, zero failed across 55 suites |
| Production Clippy | Library/binary/example checks with warnings, unwrap and expect denied passed |
| All-target Clippy | Warnings denied passed |
| Worker Python regression | 238 tests, 2 skipped, zero failures |
| Formatting and maintainability | Passed |
| Native UI | Dedicated Tailnets tab; named masked save/replace/remove flows, restart persistence and errors; dark/light themes and narrow/wide layout inspected |
| Provisioning UI | Multiple saved networks and None; cancelled creation starts nothing; allocated cloud shows a read-only provisioning choice |
| Native viewing | Connected native VNC panels, actual presentation and advancing frames on isolated desktops; 576-frame final-candidate video and 83-frame selection video fully decoded; representative feature frames inspected |
| Live MCP | Real agent panel; two saved entries returned as names/IDs only; selection schema present; raw keys and Stop selection refused without echo; no credential-reading tool |
| Selection fences | Core integration tests cover prepared selection, first-allocation confirmation, duplicate/lost-answer retry and refusal to change an allocated cloud |
| Real allocation | Minimal Linux cloud provisioned and this PC discovered in sanitized inventory |
| Cloud isolation | Agent UID 10001, no-new-privileges, zero capabilities; private node state, selection and daemon socket inaccessible; only public inventory fields visible |
| Persistence | Stop/resume retained the same Tailscale node without requiring the saved auth key |
| Daemon recovery | Killing the owned cloud daemon recovered in 8.9 seconds with the same identity; isolation and inventory rechecked |
| Worker compatibility | Final image imported a synthetic repository, prepared a shared checkout, committed and created a worktree as UID 10001 |
| Private TCP | Random nonce matched through cloud Tailscale Serve on permitted port 22, requested from this PC |

The real cloud saw itself and 42 ACL-visible peers, including this PC. This is
ACL-filtered discovery, not an administrative inventory of the entire tailnet.
HTTP proxy egress worked. Direct cloud-to-PC TCP and the alternative port 443
were blocked/timed out under existing policy. Configuring Serve on this PC was
refused by its local privilege policy. The successful private Serve test was in
the reverse direction, PC to cloud; it does **not** qualify arbitrary inbound
access to this PC. No ACL, firewall, operator setting or public Funnel was changed.
The cloud Serve configuration was restored to empty and its nonce backend removed.

The paid provider lane was Hetzner on Linux amd64. RunPod, other architectures,
macOS/Windows credential stores and graphics are not live-qualified by this run.
MCP's existing owner/checked-companion and first-allocation confirmation gates
remain; there is no general agent-controlled first-cloud creation tool. The
signed project-session runtime refuses selected tailnets until it has its own
qualified unprivileged launcher.

Long-running native capture also showed intermittent blank/glyph frames after
layout/theme interaction. The final feature was visible in the verified recording;
no terminal renderer changes or renderer qualification are claimed by this work.
Private screenshots, recordings, provider identifiers and network details are
excluded from this repository and the public issue.

## Cleanup

The owner approved deletion after the smoke. The cloud reached `Deleted`;
read-only provider queries confirmed the task's server, persistent volume and
labelled SSH key were absent. Its disposable network session had been logged out.
Task-owned canaries and the synthetic local worker container were stopped, and
private registry login material was removed. The shared PC's network policy was
unchanged. The auth-key-only scope and follow-ups are already in the issue;
publishing the later test results was blocked by an invalid CLI login and an
integration without issue-edit permission. The complete results are retained here.

## PR UI refresh

The final settings view removes the introductory credentials card and gives the
name/key fields consistent 12-pixel padding, 14-pixel text, eight-pixel corners
and larger spacing between labels, inputs and actions. Credential behavior is
unchanged. The refreshed frozen UI SHA256 is
`fb53b38b653652d8c2bcb10b3de9aef2f2558d3b15e46438254e6d2f78a1482c`;
the actual application child matched it.

A fresh isolated native VNC recording covers two synthetic saved networks, masked
save and replacement, an empty replacement-key field, selecting either network
or None in the real provisioning dialog, cancellation without allocation, and
removal. Full workspace and speech tests, formatting, maintainability and both
mandatory Clippy tiers passed again. Pedantic Clippy remains advisory and reports
existing excessive-bools warnings. No paid allocation was repeated for these
presentation-only changes.
