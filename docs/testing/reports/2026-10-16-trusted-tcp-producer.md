---
procedure: trusted-tcp-producer
date: 2026-10-17
issue: https://github.com/peters/horizon/issues/1335
status: passed
---

# Original Producer replacement — final R2 qualification

The checked code revision is `5e3093192285033f8ca529234618ab5137f9694c`. The final test executable is `b2879cc81bcb5ba692c236c473e33472dc0a156ed57409d44989389629009c26`. Subsequent changes to this report do not change executable inputs.

This default-off Linux component binds each opaque pending connection to its original listener instance. A successor at the same pathname cannot adopt it. A weak cancellation handle retains no payload descriptor and follows the same private connection through acceptance. Receipt and control installation serialize with cancellation. Cancellation closes the installed control, or discards its legitimate first-byte queued descriptor while the pending owner remains retained. Final owner drop closes any remaining malformed queue. Unpolled admission reserves finite capacity and releases both its connection and reservation on drop.

The component retains original daemon, process, socket and executable custody, bounded PROXY1/WhoIs comparison, strict descriptor receipt and opaque accepted-stream ownership. Normal completion requires exact DRAINED0x01 plus EOF. Payload write EOF preserves cancellation. The runtime prerequisite is merged in [PR #1712](https://github.com/peters/horizon/pull/1712), at Main `f0afa2c9a85f4fa6bcc801b7cf25466ee24b9e1c`.

## Local qualification

Fresh enabled Producer coverage passed 249 isolated cases and one host complement. Both separate root backend cases passed, including original-listener retirement and same-path successor refusal. Queued-rights tests cover pending drop, unpolled drop, Busy refusal and physical cancellation while the original pending owner remains retained. Other cases cover cancellation across acceptance, private acceptance comparison, strict receipt and persistent drain acknowledgement. Mechanical transport fixtures create no accepted identity.

The complete applicable offline matrix is accounted for by fresh results and consumed-input comparison. Default workspace coverage has 7,056 passes: 4,672 unchanged isolated passes, 2,371 fresh UI passes and 13 unchanged host complements. Speech coverage has 2,417 fresh passes. SDK coverage has 531 passes and two doctests; gateway, Owner and deployment-example coverage have 113, three and 21 passes. Python coverage has 730 passes with eight root-only cases locally non-applicable.

Enabled Producer and workspace blocking/strict tiers, formatting and repository checks passed. The fresh advisory sweep retains two unchanged diagnostics; it is not an all-package pedantic-clean result. Of 92 earlier product records, 80 are unchanged and reusable. Ten UI products and both enabled Producer products are replaced by fresh builds. All four normal UI variants contain all four current bundled assets. Actual compiler profiles, features, raw dependency inputs, generated asset mappings and individual result decisions are retained. A cache flag alone does not establish reuse.

Two earlier local Clippy failures remain recorded: naming/documentation/manual-async diagnostics, followed by an unused-async diagnostic. The final implementation uses bounded synchronous admission without a false await or lint suppression. All required tiers pass on the corrected inputs.

## Genuine current qualification

All four first/final cases passed once on the checked executable inputs on 2026-10-17:

| Runtime | Exchange | Half-close |
|---|---|---|
| Original Kernel daemon | 01:16 UTC, pass | 01:16 UTC, pass |
| Normal Hetzner userspace worker | 01:21 UTC, pass | 01:21 UTC, pass |

Each case proved local forgery refusal, genuine peer identity, PING/PONG, actual peer write/read EOF and original Serve/test/backend retirement. Both userspace cases also proved same-daemon SOCKS/HTTP spoof refusal. Both half-close cases required the original daemon's drain acknowledgement and EOF. These results supplement the separate root successor regression; positive cases alone do not establish successor refusal.

The normal worker used the published immutable CPU image and its original shipped Worker, CLI and daemon. Only the current test executable was transferred. Normal deletion completed at 01:21 UTC. Fresh direct checks returned 404 for all five owned provider resources. Complete inventories of seven resource kinds retained the original foreign baseline, and the normal recorded node was confirmed deleted.

Final cleanup completed at 01:22 UTC. Both local guests and their network were absent, the temporary key was revoked, both captured guest nodes were absent and the private saved binding was removed. A fresh check confirmed the original two devices, policy and catalog. No paid resource from this window remains live.

The earlier HTTP custody observation failure remains recorded with its exact branch unproved. Its bounded original-socket acceptance observer has two real-loopback regressions. Earlier successful runtime inputs remain historical and are not substituted for the R2 results.

Exact-head CI and independent PR review remain separate merge gates. This evidence qualifies only the Producer contract. It does not activate or qualify Gate admission, owned mappings, keeper composition, workspace membership, Funnel, protected publication, Native approval or public UI/CLI/MCP operations.

See the [procedure](../procedures/trusted-tcp-producer.md) and [boundary](../../architecture/trusted-tcp-producer.md). Private identities, addresses, credential references and machine paths do not belong in this public report.
