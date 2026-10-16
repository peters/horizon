# Observed RunPod TCP ports

The default-off `horizon-cloud/exposure-ports` feature decodes the existing v2
Pod `runtime.ports` response. It supplies comparison facts for later exposure
integration. It does not publish a service or change Pod creation or updates.

The response is untrusted. A mapping requires a RUNNING Pod, the existing numeric
root SSH endpoint, a nonzero TCP private and public port, and the same numeric
public address as SSH. Requested ports, HTTP proxy entries and pending mappings
are not endpoints. Runtime records are bounded to 128. Malformed, conflicting,
duplicate or unsupported port records discard all additional runtime mappings;
the original SSH endpoint remains available. IPv4 and IPv6 use numeric equality.
The decoder grants no identity, custody, readiness or firewall authority. A
controller must inspect the original Pod again and compare its captured mapping
before using it. Ports on another public address remain unsupported.

The wire shape follows the official [v2 Pod response](https://docs.runpod.io/api-reference-v2/pods/get-a-pod)
and [OpenAPI document](https://api.runpod.io/v2/openapi.json).
