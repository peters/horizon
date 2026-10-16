# Pinned Tailscale Unix PROXY v1 runtime

This directory changes Tailscale v1.102.5 only at its Serve Unix-backend boundary.
It does not add an exposure controller or enable a Horizon exposure feature.

The official tag resolves to commit `5fb2a81b065b0a0bbbfc67ab20a0d9c6a1108115`.
The [commit archive](https://codeload.github.com/tailscale/tailscale/tar.gz/5fb2a81b065b0a0bbbfc67ab20a0d9c6a1108115)
has SHA-256 `cae96823682d3ae1eaa2987ef31cf7b4bc06046933ccf1842cfd2aaf3094dba5`.
`build.sh` checks those bytes before extraction and applies `unix-proxy-v1.patch`
with zero fuzz. The archive retains the upstream `go.mod` and `go.sum` unchanged.
The upstream module requires Go 1.26.6.

Both Docker recipes pin the official `golang:1.26.6-bookworm` image index at
`sha256:116d58cbd88c1297624acc6e967a060012422bacf9930927e23fb719189c6f36`.
Its native Linux manifests are:

| Architecture | Manifest |
|---|---|
| amd64 | `sha256:433f9dc4f8ea3a1ce4e28f9f15d0f7c056b10475307f886d6f1ac1ccc4abd976` |
| arm64/v8 | `sha256:7939e2c75db3d059fc944bb6464a916d0fa64bd5a3bd7b3528f2a1ac7673a0eb` |

The native image jobs run both affected upstream test packages before building
the two binaries. The builder uses the local compiler, unchanged module checksums,
CGO disabled, trimmed paths and no embedded build ID. The version includes the
complete patch SHA-256. Bit-for-bit reproducibility still requires two independent
builds with equal inputs; a source pin alone does not prove it.

The patch permits Unix PROXY v1, reports the original virtual Serve port and refuses
Unix v2. TCP v1/v2 retain their existing destination backend port and behavior.
The new Unix path refuses a missing same-family node address or zero ingress port.
Existing Unix forwarding without PROXY remains available.

`collect-notices.py` copies upstream BSD-3-Clause licensing, the upstream dependency
index, Go licensing and full notices from the exact consumed Go modules. Their
module identities and notice paths remain in `modules.json`; the exact patch and
source hashes remain beside them. Go, source, module caches and build tools are
not copied into the runtime image. No new Go module dependency is added.

The helpers artifact now supplies these same two patched binaries and notices.
The standalone recipe builds the same runtime from these three declared inputs.
A published helper/worker digest must come from the actual two-architecture image
workflow. This change does not replace the existing quick-start pin or invent a
new published revision. This PR changes the generic helper and worker recipes only.
The development chain has a separate `.horizon/build-worker-tools.sh` helper revision
and still needs its own tested runtime rollout. `youpark.no` derives a pinned
Horizon development image and does not replace its Tailscale binaries. A later
published development-image/pin update must check that companion's actual runtime.
Its existing `scripts/verify-horizon-cloud-pins.sh` checks .NET SDK and MongoDB
versions; run it as a supplemental companion check, never as proof of Tailscale
bytes or support. The company products-map edge remains unchanged.

Read [the trust boundary](../../../docs/architecture/tailscale-unix-proxy.md)
and [the procedure](../../../docs/testing/procedures/tailscale-unix-proxy.md).
