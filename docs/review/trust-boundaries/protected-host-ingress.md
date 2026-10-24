---
applyTo: "crates/horizon-cloud/src/host.rs,crates/horizon-cloud/src/host/**,crates/horizon-cloud/src/profile.rs,crates/horizon-cloud/src/hetzner/cloud.rs,crates/horizon-core/src/cloud_runtime/deployment/replacement/**"
---

# Protected host ingress

Read `docs/architecture/protected-host-ingress.md` before reviewing this boundary.
Report concrete cases where:

- Absent/false intent changes existing host config or rebuild upgrades publication.
- SDK fabricates Worker mappings or readiness from profile intent or receipt JSON.
- An agent can bind either physical pool after a Gate dies, or private backends
  become published. Do not drop capabilities needed by existing bootstrap.
- Host inspection trusts requested ports instead of actual Docker publication,
  dumps environment/credential output or accepts partial/extra publication sets.
- Original CID, image, PID/start, pidfd, actual namespace, sysctl, effective root
  capability or readonly root-owned generation checks are absent or inconsistent.
- A receipt can be overwritten, refresh a generation, rehydrate authority or
  imply original source preservation without actual external qualification.
- Cancellation/readback failure prevents exact original cleanup, trusts a
  successor, blocks indefinitely or claims absence after failed daemon I/O.
- Tests execute START/GUARD/SETUP on the desktop host or mount its socket/proc
  into a worker. Real tests belong only in an owned isolated VM.
