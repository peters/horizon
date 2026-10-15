# Check observed RunPod TCP ports

This procedure tests the SDK decoder with a local scripted HTTP server. It does
not rent a Pod, publish a service or qualify worker isolation.

1. Run `cargo test -p horizon-cloud runpod::tests::ports`.
   Result: With the feature off, the original SSH mapping remains the only map.
2. Run `cargo test -p horizon-cloud --features exposure-ports`.
   Result: IPv4 and IPv6 live TCP mappings use the actual returned public port.
   SSH remains unchanged. Requested, pending and HTTP ports are not endpoints.
   Malformed, duplicate, conflicting, foreign-address and oversized runtime
   records supply no additional endpoints. Stopped Pods and DNS SSH hosts do not
   promote runtime ports.
3. Run the applicable blocking and strict Clippy tiers with `exposure-ports`.
   Result: Both tiers pass.

Keep the checked source, commands, output and toolchain in the qualification
record. Feature activation and original-Pod refresh checks require separate
controller evidence. No live RunPod test is required for this decoder.
