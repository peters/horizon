---
procedure: trusted-exposure-gate
date: 2026-10-17
issue: https://github.com/peters/horizon/issues/1335
status: passed
---

# Trusted exposure gate — component qualification

The checked code revision is `f2fb03878da2ba38df3acd4df7fc27e2ba832b4a`, with source tree `4ff21cbe508feb57ac178452a62348b82c72956f`. Subsequent report changes do not change executable inputs.

| Executable | Features | Profile | SHA-256 |
|---|---|---|---|
| Gate test used for genuine cases | `exposure-gate`, `trusted-exposure-gate`, `trusted-tcp-producer` | debug, `test=true` | `df91436b7ea8a2fa1a803436b2acfc38b52cb606fce06e5d1f6c61a3cebf0fdc` |
| Separately built normal Gate component | same three features | debug, `test=false` | `35efbccb13ad85155df398aebbaa1eb542778ac2a14b31eb3525cfe014da3b1b` |

The genuine cases execute the component test. The normal CLI deployed the published CPU image `ghcr.io/peters/horizon-worker-base@sha256:f9f82545795762cbb41a9ee1b6899cd0c3f874952ebe190921e2af9659311cc3` and reached Ready. Its shipped Worker, CLI and daemon were retained. Original endpoint and pinned SSH checks passed. Only the component test was uploaded. This does not qualify activation of the Gate in the shipped Worker.

## Local qualification

The applicable matrix passed through fresh checks and explicit consumed-input reuse:

| Coverage | Fresh | Valid reused |
|---|---:|---:|
| Default isolated roots | 4 roots, 2,580 passes | 59 roots, 4,463 passes |
| Default host complements | 1 | 12 |
| Speech UI | 3 roots, 2,417 passes | 0 |
| Python | 0 | 730 passes; 8 root-only cases locally non-applicable |

Default coverage totals 7,056 passes. Fresh Gate coverage passed 289 isolated cases, one host complement and two separate root backend cases. Producer-only and CIDR-only coverage passed 249 and 234 isolated cases, each with its host complement. SDK coverage retains 531 passes and two doctests; gateway, Owner and deployment-example coverage retain 113, three and 21 passes. Their source, feature, dependency and runtime inputs match. Component groups overlap workspace coverage and must not be added into one aggregate count.

All component and workspace blocking/strict tiers, formatting and applicable repository checks passed. Four normal UI variants each contain all four current bundled guidance assets. Actual compiler profiles, features, raw dependency inputs, executable hashes and individual result mappings are retained. The advisory sweep stopped with two unchanged inherited pedantic diagnostics and the existing unknown-lint warning. It is not a completed clean pedantic scan. Superseded compile and prelaunch failures remain recorded.

## Genuine policy checks

All twenty cases passed once: ten with an original Kernel daemon, then ten on the normal userspace Worker. Each mode uses an external foreground Serve mapping to the original private Unix backend. These are not OwnedIngress checks.

| Policy | Kernel | Userspace Worker |
|---|---|---|
| DNS only, allow | PASS | PASS |
| Tag only, allow | PASS | PASS |
| Wrong full DNS, deny | PASS | PASS |
| Absent tag, deny | PASS | PASS |
| Mixed rules, DNS match | PASS | PASS |
| Mixed rules, tag match | PASS | PASS |
| Mixed rules, CIDR match | PASS | PASS |
| Mixed rules, all miss | PASS | PASS |
| Uppercase full DNS with trailing dot, allow | PASS | PASS |
| Tag prefix lookalike, deny | PASS | PASS |

Every case verifies exactly one original peer and compares its actual StableID. All twelve allows prove PING/PONG/DONE, peer write shutdown and EOF, one successful original DRAINED completion, and twelve charged and forwarded bytes before Gate close. These numeric values are source assertions exercised by the exact selector, not separately printed counters. All eight denies require the same verified peer and exactly one policy-denial increment. An unrelated refusal cannot satisfy the denial check.

Every case confirms original Serve and test-process retirement, empty Serve configuration, absent private backend and empty component listener ports before the next case. There were no retries or cleanup errors.

## Cleanup and limits

Normal deletion completed at 02:28 UTC. All five exact provider resources returned 404. Complete seven-kind inventories retained the original foreign baseline, and the recorded normal node was deleted. Final cleanup completed at 02:29 UTC: both original local guests and their network were absent, the temporary key was revoked, both captured guest nodes were absent, and the private saved binding was removed. Fresh checks preserved the original two devices, one user, policy and catalog. All owned resources were removed within their original deadlines.

The component remains off by default and supports Linux. Named sources use the original Producer and opaque accepted stream; direct CIDR traffic retains TCP. This result does not qualify OwnedIngress, keeper renewal, protected publication, provider source preservation, Funnel, workspace membership isolation or public UI/CLI/MCP activation. Native interaction and GIF acceptance remain separate M1/M2 requirements. Exact-head PR checks and independent review remain merge gates.

See the [procedure](../procedures/trusted-exposure-gate.md) and [trust boundary](../../architecture/trusted-exposure-gate.md).
