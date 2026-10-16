---
applyTo: "crates/horizon-core/src/cloud_runtime/repository/source.rs,crates/horizon-core/src/dir_search.rs,crates/horizon-core/src/dir_search/**,crates/horizon-ui/src/app/cloud_panel/production/creation/source.rs,crates/horizon-ui/src/app/cloud_panel/production/creation/source/**"
---

# Checkout search boundary

Read `docs/architecture/checkout-search.md` for the guarantees and limits.

- Treat every home-folder entry and every Git metadata file as untrusted: it can be a
  link, a loop, a pipe, a device, huge, or change while it is read.
- The checkout search must start no process and must stay on its background thread.
- Keep the visit, depth, checkout and file-size bounds, and apply each bound to the bytes
  actually read, not only to a size checked before the read.
- On macOS, never open anything inside a guarded home folder, also through a link.
- Flag UI-thread file system work in the finder and the source field that is not bounded.
- Update the boundary document in the same change when a guarantee or limit changes.
