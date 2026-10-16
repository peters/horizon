---
applyTo: "crates/horizon-cloud-worker/src/protected_ingress/**,crates/horizon-cloud-worker/src/bootstrap.rs,crates/horizon-cloud-worker/src/main.rs,crates/horizon-cloud-worker/Cargo.toml"
---

# Protected host receipt reader boundary

Read `docs/architecture/protected-ingress-reader.md` before review.
The default-off reader retains original local custody; exported receipt facts remain passive.
No command or runtime path activates this component.

Report these failures:

- A path, request JSON, environment fallback or caller allocation mints a live reader.
- The reader uses a private receipt parser instead of canonical Protocol validation.
- A symlink, writable mount, non-root file, broad mode, extra link or oversized read is accepted.
- Revalidation omits held or freshly opened metadata, timestamps, content or fixed-path identity.
- A successor file, generation, PID 1 or namespace replaces original custody.
- Capability readback describes another thread instead of the caller.
- Rootless/remapped UID/GID maps or mismatched PID-1 user namespaces pass this host-root lane.
- Actual PID-1 start ticks, network namespace or sysctl fail to match the receipt.
- A failed reader regains validity, or serialized facts recreate its authority.
- Receipt publication is described as source preservation, named admission or owner approval.
- Refusal selects high ports, starts a Gate, or activates a public operation.

H03 requires the original task-owned VM, genuine host mount and reviewed command plan.
An ignored test, pure metadata test or desktop execution cannot qualify actual root readback.
