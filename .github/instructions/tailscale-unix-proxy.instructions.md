---
applyTo: "examples/cloud-worker/Dockerfile,examples/cloud-worker/Dockerfile.helpers,examples/cloud-worker/tailscale/**,examples/cloud-worker/prepare-context.py,examples/cloud-worker/test_prepare_context.py,examples/cloud-worker/.dockerignore"
---

Read docs/architecture/tailscale-unix-proxy.md. Review the checked-in upstream
patch as executable production and test source, not as generated text.
Preserve exact archive/compiler pins and unchanged Go module checksums. Go belongs
only in build stages. Both native architectures must run affected upstream tests
and retain complete consumed-module notices before publication.

Unix PROXY must accept v1 only, use the actual virtual Serve port and write its
header before application bytes. Missing node-family address and invalid port
must refuse. Keep existing TCP v1/v2 and ordinary Unix forwarding unchanged.
A rejected CLI update must retain the original config. Verify actual Unix dial,
header bytes, bidirectional payload and refusal, not patch-text matching tests.

This runtime addition grants no source authority. Subsequent consumers require
original root-private directory/listener and accepted Unix peer credentials from
the original pinned daemon, plus lifecycle/generation checks. Do not substitute
same-PID loopback TCP, a TCP bridge, saved JSON or generic-proxy output. No exposure
activation, fabricated publication pin or transfer of old TCP security evidence.
