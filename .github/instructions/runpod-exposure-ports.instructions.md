---
applyTo: "crates/horizon-cloud/src/runpod/**"
---

Review the default-off `exposure-ports` decoder against
`docs/architecture/runpod-exposure-ports.md`. Provider runtime data is untrusted
comparison data. Requested ports are not live mappings. Require a RUNNING Pod,
nonzero TCP port pairs and the numeric SSH public address. Ambiguous runtime data
must not replace the original SSH endpoint or supply an exposure endpoint.
This component does not change provisioning or qualify public readiness.
