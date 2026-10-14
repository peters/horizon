---
applyTo: "crates/horizon-core/src/cloud_runtime/settings.rs,crates/horizon-ui/src/app/cloud_panel/production/preparation/job.rs,crates/horizon-ui/src/app/cloud_panel/production/lifecycle.rs,crates/horizon-ui/src/app/cloud_panel/production/rebuild.rs,crates/horizon-ui/src/app/cloud_panel/production/resize.rs,crates/horizon-ui/src/app/cloud_panel/production/close.rs,crates/horizon-core/src/cloud_runtime/repository/source/transport.rs,crates/horizon-cloud/src/github/pulls.rs"
---

# Clouds for untrusted code

An Upkeep test cloud runs the code of a pull request. Its folder holds the `isolated`
marker (`settings::mark_isolated`) before its first deployment. `docs/upkeep.md`,
section "Trust boundary of the test cloud", describes the boundary. Check that:

- Every path that builds a deployment `Request`, reconnects a worker or talks to it
  from cloud settings calls `Settings::isolate_for` with the cloud's folder, on a
  worker thread: the deployment preparation, a resume, a rebuild, a resize, a
  companion resume and the companion refresh. A new path does too.
- `isolate_for` takes away GitHub access and the Git and BrowserStack credentials, and
  a marker that cannot be read counts as there.
- `close_cloud_unasked` closes only as the confirmed close dialog does.
- `fetch_commits` changes no ref, no working tree and no `FETCH_HEAD` of the person's
  checkout, never removes a lock there, and starts no program that the checkout's
  configuration names (only HTTPS, no credential helper, monitor or hook).
- `Client::pull_request` checks the commits and names it returns.
