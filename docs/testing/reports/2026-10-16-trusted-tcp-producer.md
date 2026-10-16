---
procedure: trusted-tcp-producer
date: 2026-10-17
issue: https://github.com/peters/horizon/issues/1335
status: incomplete
---

# Original Producer replacement — offline qualification complete

The replacement is a default-off Linux Producer-only component. It uses the original root-private Unix listener and kernel daemon custody, bounded PROXY1/WhoIs comparison and an opaque AcceptedStream retaining its private descriptor cancellation endpoint. Normal completion requires the original daemon's exact DRAINED0x01 followed by EOF; write-half EOF preserves control. This component adds no Gate adapter, owned mapping, keeper, Funnel, protected publication, enrollment, Native approval or public interface activation.

The runtime prerequisite merged as [PR #1712](https://github.com/peters/horizon/pull/1712). This candidate is based on actual Main `f0afa2c9a85f4fa6bcc801b7cf25466ee24b9e1c`. The complete local matrix ran in the same checkout before that merge against the prospective runtime source. Reanchoring preserves all twelve Producer Rust leaves, dependency and toolchain inputs, features, normal embedded guidance and executable bytes. The final runtime patch and its procedure are compared separately; old runtime image or daemon observations do not qualify the merged runtime. Postmerge Main workflows and the normal published image remain separate gates and are not claimed complete here.

## Local qualification

The current offline evidence covers 63 default roots: 7,043 isolated passes and 13 host complements, for 7,056 passes, with 55 declared ignores in the isolated run. Speech adds 2,417 passes and three ignores. The enabled Producer suite has 239 isolated passes plus its host complement, for 240 passes; its original private-listener lifecycle test also passes separately in the isolated root fixture. Selected SDK tests pass 531 cases and two doctests; gateway and Owner cases pass 113 and three respectively. Python coverage is 730 passes with eight root-only cases explicitly not applicable locally.

Workspace and enabled Producer blocking and strict tiers pass, along with formatting, maintainability, paired skills, CI command contracts and documentation checks. The advisory tier retains two unchanged dependency findings; it is not an all-package pedantic-clean result. Actual compiler profiles, features, dependency inputs, normal companions and 92 frozen product records are retained. Four normal UI variants each contain all four current bundled assets. Results are reused after input comparison; a cache flag alone does not establish equivalence. No compiler or runtime test failure occurred in that matrix. Earlier conservative input-reuse and whitespace postprocessor refusals remain recorded as equipment observations.

## Genuine runtime qualification pending

PR #1711's earlier TCP implementation passed two Kernel and two userspace observations, but its first review found a missing boundary: the same daemon's generic SOCKS or HTTP proxy could reach its TCP backend and forward an attacker-written header. Those earlier observations do not qualify source attribution or the replacement Unix/FD contract. Their exact owned-cleanup evidence remains historical.

The replacement still requires genuine Kernel and userspace primary and half-close cases on the first reviewable and final candidate, using a newly started original daemon with the current runtime baked into its normal image. Same-daemon proxy spoof refusal, genuine peer identity, payload and EOF must be observed separately from the daemon's acknowledgement. No custody-refusal outcome counts as a positive half-close result. Retain exact child/listener/provider/node/private-binding cleanup and unchanged protected baselines. A stock or previously qualified image, active binary overlay, unit fixture, published executable alone or old real-daemon success cannot establish this gate. Exact-head CI and independent PR review remain separate requirements.

See the [procedure](../procedures/trusted-tcp-producer.md) and [boundary](../../architecture/trusted-tcp-producer.md). Private identities, addresses, credential references and machine paths do not belong in this public report.
