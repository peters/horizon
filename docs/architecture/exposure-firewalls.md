# Exposure firewall provider boundary

The Cloud `exposure-firewall` feature is off by default. It adds exact Hetzner
firewall API operations. It does not enable service exposure or change provisioning.

The caller supplies expected operation, allocation and numeric server identities.
These values are comparison facts, not approval. Every mutation of an existing
firewall reads its exact numeric ID again and checks its name and full labels.
Only no attachment or one exact server attachment is accepted. A selector, another
server, duplicate targets, unknown resource fields or additional labels refuse.
Creation and application also read and verify the original allocation's server.
Removal can retire an original attachment after its server has disappeared.

Provider HTTP responses are untrusted and bounded by the shared adapter. Rule
decoding refuses unknown fields. Replacement uses the complete supplied rules;
no read-modify-write of a foreign firewall is supported. Invalid rules, IDs or
action batches fail. The API check uses the current primary OpenAPI specification.

Before create, the owner controller must durably save Requested and the exact
credential binding. Creation sends one POST with no attachments. A lost or malformed
response remains uncertain; it never triggers another create. Reconciliation uses
the original operation label and refuses ambiguous or foreign candidates. The
returned ID must be saved before waiting for actions. Action success is not proof
of current attachment or rules; use fresh provider observations afterward.

The provider API has no atomic compare-and-swap for this sequence. External changes
between preflight and a mutation remain possible. The final controller must retain
the original resource ownership, compare readbacks and physically close its worker
gate on uncertainty. This SDK is not a physical-release proof or an approval grant.

The [management controller](exposure-management.md) inspects every currently
attached firewall and retains complete original policy. Its caller must supply
the actual approved management rules and verify that they cover pinned SSH access. Adding a firewall can restrict previously unrestricted
inbound traffic. Multiple firewalls combine their allowed traffic, so an extra
wide SSH rule can widen another firewall's policy. Rules do not filter private
Hetzner networks or terminate established connections. Therefore worker admission
and physical expiry remain necessary.

These constraints follow the [Hetzner firewall FAQ](https://docs.hetzner.com/cloud/firewalls/faq/).
The current API surface is checked against
[Hetzner's OpenAPI specification](https://docs.hetzner.cloud/cloud.spec.json).
Actual management preservation, original source attribution, provider lifecycle
integration and RunPod port mappings remain separate qualification requirements.
