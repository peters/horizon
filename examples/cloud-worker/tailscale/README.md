# Pinned Tailscale Unix PROXY v1 runtime

This directory changes Tailscale v1.102.5 at its Serve Unix-backend boundary
and raises the versions of some `golang.org/x` modules so that it builds with Go 1.27.
It does not add an exposure controller or enable a Horizon exposure feature.

The official tag resolves to commit `5fb2a81b065b0a0bbbfc67ab20a0d9c6a1108115`.
The [commit archive](https://codeload.github.com/tailscale/tailscale/tar.gz/5fb2a81b065b0a0bbbfc67ab20a0d9c6a1108115)
has SHA-256 `cae96823682d3ae1eaa2987ef31cf7b4bc06046933ccf1842cfd2aaf3094dba5`.
`build.sh` checks those bytes before extraction and applies `unix-proxy-v1.patch`
with zero fuzz. The upstream module requires Go 1.26.6.

The patch also changes `go.mod` and `go.sum`. It raises `golang.org/x/tools` from
v0.47.0 to v0.50.0. Go 1.27 writes export data version 5, and versions before v0.50.0
cannot read it: `go generate ./ipn` then writes `invalid type` into `ipn_view.go`
and fails. The same change raises `crypto`, `mod`, `net`, `sync`, `sys`, `telemetry`,
`term` and `text` to the minimum versions that `golang.org/x/tools` v0.50.0 requires.
These are the versions that upstream v1.104.0 uses. Upstream raised `golang.org/x/net`
to v0.58.0 to fix an HTTP/2 deadlock under Go 1.27 (commit `de9ec7ee`); v0.59.0 has it.
`go get golang.org/x/tools@v0.50.0` and `go mod tidy` with Go 1.26.6 and the public
checksum database made the change. It adds no module. The recipe builds with Go 1.26.6
and with Go 1.27.

Both Docker recipes pin the official `golang:1.26.6-bookworm` image index at
`sha256:116d58cbd88c1297624acc6e967a060012422bacf9930927e23fb719189c6f36`.
Its native Linux manifests are:

| Architecture | Manifest |
|---|---|
| amd64 | `sha256:433f9dc4f8ea3a1ce4e28f9f15d0f7c056b10475307f886d6f1ac1ccc4abd976` |
| arm64/v8 | `sha256:7939e2c75db3d059fc944bb6464a916d0fa64bd5a3bd7b3528f2a1ac7673a0eb` |

The native image jobs run complete `./ipn`, `./ipn/ipnlocal` and
`./cmd/tailscale/cli` test packages before building
the two binaries. The builder uses the local compiler, unchanged module checksums,
CGO disabled, trimmed paths and no embedded build ID. The version includes the
complete patch SHA-256. Bit-for-bit reproducibility still requires two independent
builds with equal inputs; a source pin alone does not prove it.

The patch permits Unix PROXY v1, reports the original virtual Serve port and refuses
Unix v2. TCP v1/v2 retain their existing destination backend port and behavior.
The new Unix path refuses a missing same-family node address or zero ingress port.
Existing Unix forwarding without PROXY remains available.

The additional Serve-only `--horizon-flow-cancel` boolean defaults false.
JSON `TCPPortHandler.HorizonFlowCancel` is omitted when false. Opt-in requires
Linux raw TCP, an absolute filesystem Unix backend, PROXY1 and nonzero ingress;
HTTP/HTTPS, TLS termination and Funnel refuse. CLI, direct LocalAPI recursive
config and restored handlers validate it; rejected updates retain prior config.
Ordinary false/absent paths keep their existing bytes and behavior.

An opted-in handler sends header byte `P` once with one SCM_RIGHTS unnamed Unix
stream cancellation endpoint, closes its local sent-end copy, then sends the
remaining header and payload. Payload write EOF half-closes only that direction.
Consumer control EOF/data/error cancels both payload directions; the consumer
never sends control bytes or half-closes control. Only after both copies and
required CloseWrite calls succeed does the daemon send control `DRAINED=0x01`,
close control and join its reader. Consumer finish requires exactly that byte
then EOF under existing deadlines. No ambiguous FD/ACK retry or hidden duplicate
is permitted. All sender/copy/control goroutines and original FDs retire.
ACK means daemon copying/frontend half-close, not remote app delivery.

This capability requires the exact qualified pair and original root-controlled
private listener/daemon custody; configuration and descriptors alone grant no
identity. Actual Producer/Gate Kernel and userspace Worker consumption remains
a separate gate. Earlier non-descriptor image/package/reproducibility evidence
cannot qualify the new protocol; current-head native and independent-build
results must be recorded before claiming them.

`collect-notices.py` copies upstream BSD-3-Clause licensing, the upstream dependency
index, Go licensing and full notices from the exact consumed Go modules. Their
module identities and notice paths remain in `modules.json`; the exact patch and
source hashes remain beside them. Go, source, module caches and build tools are
not copied into the runtime image.

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
