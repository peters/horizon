# Exposure policy facts

The optional `horizon-cloud-protocol/exposure-gate` feature provides pure policy
and route types. The feature is off by default. This prerequisite adds no worker
listener, owner controller, provider mutation, UI action, CLI operation or MCP tool.

## Untrusted inputs and validation

Treat JSON, saved policy, CIDR text, DNS names, tags and URLs as untrusted facts.
`ExposureRequest::from_json` decodes at most 16 KiB and refuses unknown or duplicate
fields. Call `validate` separately before use. An absent mode selects `Off`.
`Restricted` requires at least one source rule. Other modes refuse source rules.
There are at most 32 source rules and 128 total connections. Per-source limits
must be positive and must not exceed the total limit.

A target port cannot be zero, a management port, or a port in the reserved
20000–20063 range. The blocked management ports are 22, 5900, 9222, 2375, 1055,
1056, 47280 and 47281. Reserved ingress ports are 20000–20031. Reserved private
backend ports are 20032–20063. The backend calculation supplies facts only.

CIDR matching uses exact address families and prefix bounds. An IPv4-mapped IPv6
address uses the IPv4 rules. Full tailnet DNS rules use normalized names. Tag
rules use exact names and case. Mixed rules use OR semantics after all rules
pass validation. A malformed rule refuses the complete policy.

## Trust boundary

`allows` compares only an IP address. It never accepts `TailnetOnly` or a
name-only restriction. `matches_tailnet` compares caller-supplied IP, DNS and tag
facts. Its return value does not authenticate those facts. A future live owner
must bind them to the original authenticated accepted connection and current
workspace members. A tailnet-range IP, a saved name or an API inventory is not
that proof. A parsed policy grants no owner approval, credential access, path
access, runtime handle or permission to publish a service.

`Route::Gate` is the default route. `FunnelHttps` requires an explicit `Public`
policy and HTTPS port 443, 8443 or 10000. A refused route never selects a raw TCP
fallback. Endpoint helpers require the original canonical full DNS name. Their
TCP or HTTPS URL is comparison data, not proof of source policy, a live listener,
provider ingress, TLS or public readiness.

The eight-hour constant belongs to the later M2 service lifecycle. This module
runs no timer and grants no renewal. M1 one-device sharing has no expiry.

## Delivery and validation

The Core durable-record and version-migration outcome follows this prerequisite.
It must preserve Main publication, enrollment and one-device admission records.
Worker, provider, native owner and public interface outcomes remain separate.
See [the procedure](../testing/procedures/exposure-policy.md) for the pure tests.
Component results do not complete issue #1335 or qualify real ingress.
