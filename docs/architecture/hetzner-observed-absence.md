# Hetzner server absence observation

The SDK function `hetzner::cloud::delete_observed` adds an optional observation to
the existing cloud deletion operation. The existing `delete` API keeps its
signature and its deletion rules. No Horizon UI, CLI or MCP operation uses the
new result in this change.

## Trust boundary

The caller supplies the original operation record and the `Records` callbacks.
The provider supplies untrusted HTTP responses. The SDK checks the response
format, exact server ID, operation name and ownership label before deletion.

The SDK returns `Some(OriginalServerAbsence)` only when all these conditions hold:

- The operation was `Bound` at the start of this call.
- The recorded server ID was a canonical, nonzero decimal ID.
- The same immutable `Hetzner` client observed that exact server and its ownership.
- That client then received HTTP 404 for the exact server.
- Server, volume and SSH-key cleanup completed under the existing deletion rules.
- All required operation and journal saves succeeded.

An initial HTTP 404 returns no observation. `Prepared`, resolved `Requested`
history and `Terminated` retries return no observation. Another client's initial
HTTP 404 cannot inherit a positive observation from an earlier call.
An operation failure, including an observed cancellation, returns an error and
no observation. A provider refusal, a lost reply or a failed save also returns
no observation. Existing records remain the source for a later retry.

## Result and limits

`OriginalServerAbsence` exposes only `operation_id()` and `server_id()`.
Its fields are private. It has no public constructor, `Clone` implementation or
serialization support. A caller cannot reconstruct it from a path or saved IDs.
The observation describes this completed call; it is not a credential or a
durable credential identity.

The observation alone does not release a tailnet address, remove a device or
grant permission for another provider action. No durable consumer exists in
this change. A future consumer must bind its own original records and authority
before it can use this result. No Core call site or public interface changes.

The automated checks use a loopback HTTP fixture with synthetic credentials.
They do not qualify a real Hetzner project or a real provider deletion.
Use the [test procedure](../testing/procedures/hetzner-observed-absence.md)
for the SDK contract checks.
